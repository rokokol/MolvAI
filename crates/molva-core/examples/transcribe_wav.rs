// SPDX-License-Identifier: MIT
//! Manual check of the recognition pipeline on a WAV file
//!
//! ```text
//! cargo run -p molva-core --example transcribe_wav -- tests/fixtures/privet_ru.wav \
//!     [--model PATH] [--language ru] [--threads N] [--timecodes] [--no-trim]
//! ```
//!
//! From the microphone (an end-to-end check of the capture together with the recognition):
//!
//! ```text
//! cargo run -p molva-core --example transcribe_wav -- --mic 5 [--device NAME] --language ru
//! ```
//!
//! Prints the text to stdout, and the timings and service output to stderr (Y-15)

// The example is a command-line utility: the recognized text is its result on stdout
#![allow(clippy::print_stdout)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use molva_core::app::audio::trim_silence;
use molva_core::domain::audio::{downmix_to_mono, AudioSource, PcmAudio};
use molva_core::domain::stt::{LanguageHint, SttEngine, SttOptions};
use molva_core::infra::audio::CpalSource;
use molva_core::infra::stt::{transcribe_with_language_policy, WhisperEngine};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    wav: Option<PathBuf>,
    /// Record this many seconds from the microphone instead of reading a file
    mic_secs: Option<u32>,
    device: String,
    model: PathBuf,
    language: String,
    threads: usize,
    timecodes: bool,
    trim: bool,
}

fn run() -> Result<(), String> {
    // The whisper.cpp and core logs show with RUST_LOG=info (or debug); by default they are silent
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = parse_args()?;

    let audio = if let Some(secs) = args.mic_secs {
        record_from_mic(&args.device, secs)?
    } else {
        let wav = args
            .wav
            .as_ref()
            .ok_or("give a WAV file or --mic SECONDS: transcribe_wav file.wav [--model PATH]")?;
        let audio = read_wav(wav)?;
        eprintln!(
            "file: {} — {:.2} s, {} Hz",
            wav.display(),
            audio.duration_secs(),
            audio.sample_rate
        );
        audio
    };

    let ready = audio.to_16k();
    let ready = if args.trim {
        let trimmed = trim_silence(&ready, -45.0, 200);
        eprintln!(
            "after silence trimming: {:.2} s (was {:.2} s)",
            trimmed.duration_secs(),
            ready.duration_secs()
        );
        trimmed
    } else {
        ready
    };
    if ready.samples.is_empty() {
        return Err("no audio left after silence trimming: the file has no speech".into());
    }

    let model_name = args
        .model
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("whisper")
        .trim_start_matches("ggml-")
        .to_string();
    let mut engine = WhisperEngine::new(args.model.clone(), model_name, args.threads);

    let options = SttOptions {
        language: LanguageHint::parse(&args.language),
        timestamps: args.timecodes,
        threads: args.threads,
        ..SttOptions::default()
    };

    let load_started = Instant::now();
    let first = transcribe_with_language_policy(&mut engine, &ready, &options)
        .map_err(|e| e.to_string())?;
    let first_elapsed = load_started.elapsed();

    // The second run shows the latency on an already loaded model — this is how the daemon works
    let warm_started = Instant::now();
    let second = transcribe_with_language_policy(&mut engine, &ready, &options)
        .map_err(|e| e.to_string())?;
    let warm_elapsed = warm_started.elapsed();

    println!("{}", second.text);
    if args.timecodes {
        for segment in &second.segments {
            println!(
                "[{:>6} — {:>6}] {}",
                segment.start_ms, segment.end_ms, segment.text
            );
        }
    }

    eprintln!(
        "language: {}, no_speech_prob: {}",
        second.detected_language.as_deref().unwrap_or("?"),
        second
            .no_speech_prob
            .map_or_else(|| "?".into(), |p| format!("{p:.3}"))
    );
    eprintln!(
        "first run (with the model load): {} ms; second (model in memory): {} ms; audio {:.2} s",
        first_elapsed.as_millis(),
        warm_elapsed.as_millis(),
        ready.duration_secs()
    );
    if first.text != second.text {
        eprintln!("warning: the runs differ, the first gave: {}", first.text);
    }

    engine.unload();
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut wav = None;
    let mut model = None;
    let mut language = "auto".to_string();
    let mut threads = 0usize;
    let mut timecodes = false;
    let mut trim = true;
    let mut mic_secs = None;
    let mut device = "default".to_string();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--model" => {
                model = Some(PathBuf::from(
                    it.next().ok_or("--model needs a path to the model file")?,
                ));
            }
            "--mic" => {
                mic_secs = Some(
                    it.next()
                        .ok_or("--mic needs a number of seconds")?
                        .parse()
                        .map_err(|_| "--mic: a number of seconds is expected")?,
                );
            }
            "--device" => device = it.next().ok_or("--device needs a device name")?,
            "--language" => language = it.next().ok_or("--language needs a language code")?,
            "--threads" => {
                threads = it
                    .next()
                    .ok_or("--threads needs a number")?
                    .parse()
                    .map_err(|_| "--threads: a number is expected")?;
            }
            "--timecodes" => timecodes = true,
            "--no-trim" => trim = false,
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other => wav = Some(PathBuf::from(other)),
        }
    }

    Ok(Args {
        wav,
        mic_secs,
        device,
        model: model.unwrap_or_else(default_model_path),
        language,
        threads,
        timecodes,
        trim,
    })
}

/// Record a given number of seconds from the microphone — an end-to-end check of `CpalSource`
fn record_from_mic(device: &str, secs: u32) -> Result<PcmAudio, String> {
    let (level_tx, level_rx) = std::sync::mpsc::channel();
    let mut source = CpalSource::new(device, 1.0, secs.max(1));

    source.start(Some(level_tx)).map_err(|e| e.to_string())?;
    eprintln!("speak… recording {secs} s");
    std::thread::sleep(std::time::Duration::from_secs(u64::from(secs.max(1))));
    let audio = source.stop().map_err(|e| e.to_string())?;

    let levels: Vec<f32> = level_rx.try_iter().collect();
    let peak = levels.iter().copied().fold(0.0_f32, f32::max);
    eprintln!(
        "recorded {:.2} s at {} Hz, level measurements {}, peak RMS {peak:.4}",
        audio.duration_secs(),
        audio.sample_rate,
        levels.len()
    );
    Ok(audio)
}

/// The default model is `small` in the data directory of the user
fn default_model_path() -> PathBuf {
    directories::BaseDirs::new()
        .map_or_else(|| PathBuf::from("."), |dirs| dirs.data_dir().to_path_buf())
        .join("molva/models/ggml-small.bin")
}

/// Read a WAV of any bit depth and mix it down to mono
fn read_wav(path: &PathBuf) -> Result<PcmAudio, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        (hound::SampleFormat::Int, bits) => {
            let scale = (1i64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };

    Ok(PcmAudio::new(
        downmix_to_mono(&interleaved, spec.channels),
        spec.sample_rate,
    ))
}
