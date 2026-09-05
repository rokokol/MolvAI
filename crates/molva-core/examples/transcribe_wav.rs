// SPDX-License-Identifier: MIT
//! Ручная проверка конвейера распознавания на WAV-файле.
//!
//! ```text
//! cargo run -p molva-core --example transcribe_wav -- tests/fixtures/privet_ru.wav \
//!     [--model PATH] [--language ru] [--threads N] [--timecodes] [--no-trim]
//! ```
//!
//! С микрофона (сквозная проверка захвата вместе с распознаванием):
//!
//! ```text
//! cargo run -p molva-core --example transcribe_wav -- --mic 5 [--device NAME] --language ru
//! ```
//!
//! Печатает текст в stdout, тайминги и служебное — в stderr (Y-15).

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
        Err(err) => {
            eprintln!("ошибка: {err}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    wav: Option<PathBuf>,
    /// Записать столько секунд с микрофона вместо чтения файла.
    mic_secs: Option<u32>,
    device: String,
    model: PathBuf,
    language: String,
    threads: usize,
    timecodes: bool,
    trim: bool,
}

fn run() -> Result<(), String> {
    // Логи whisper.cpp и ядра видны при RUST_LOG=info (или debug); по умолчанию молчим.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = parse_args()?;

    let audio = match args.mic_secs {
        Some(secs) => record_from_mic(&args.device, secs)?,
        None => {
            let wav = args.wav.as_ref().ok_or(
                "укажите WAV-файл или --mic СЕКУНДЫ: transcribe_wav file.wav [--model PATH]",
            )?;
            let audio = read_wav(wav)?;
            eprintln!(
                "файл: {} — {:.2} с, {} Гц",
                wav.display(),
                audio.duration_secs(),
                audio.sample_rate
            );
            audio
        }
    };

    let ready = audio.to_16k();
    let ready = if args.trim {
        let trimmed = trim_silence(&ready, -45.0, 200);
        eprintln!(
            "после обрезки тишины: {:.2} с (было {:.2} с)",
            trimmed.duration_secs(),
            ready.duration_secs()
        );
        trimmed
    } else {
        ready
    };
    if ready.samples.is_empty() {
        return Err("после обрезки тишины не осталось звука: в файле нет речи".into());
    }

    let model_name = args
        .model
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("whisper")
        .trim_start_matches("ggml-")
        .to_string();
    let mut engine = WhisperEngine::new(args.model.clone(), model_name, args.threads);

    let opts = SttOptions {
        language: LanguageHint::parse(&args.language),
        timestamps: args.timecodes,
        threads: args.threads,
        ..SttOptions::default()
    };

    let load_started = Instant::now();
    let first =
        transcribe_with_language_policy(&mut engine, &ready, &opts).map_err(|e| e.to_string())?;
    let first_elapsed = load_started.elapsed();

    // Второй прогон показывает задержку на уже загруженной модели — так работает демон.
    let warm_started = Instant::now();
    let second =
        transcribe_with_language_policy(&mut engine, &ready, &opts).map_err(|e| e.to_string())?;
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
        "язык: {}, no_speech_prob: {}",
        second.detected_language.as_deref().unwrap_or("?"),
        second
            .no_speech_prob
            .map(|p| format!("{p:.3}"))
            .unwrap_or_else(|| "?".into())
    );
    eprintln!(
        "первый прогон (с загрузкой модели): {} мс; второй (модель в памяти): {} мс; аудио {:.2} с",
        first_elapsed.as_millis(),
        warm_elapsed.as_millis(),
        ready.duration_secs()
    );
    if first.text != second.text {
        eprintln!("внимание: прогоны разошлись, первый дал: {}", first.text);
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
                    it.next().ok_or("--model требует путь к файлу модели")?,
                ))
            }
            "--mic" => {
                mic_secs = Some(
                    it.next()
                        .ok_or("--mic требует число секунд")?
                        .parse()
                        .map_err(|_| "--mic: ожидалось число секунд")?,
                )
            }
            "--device" => device = it.next().ok_or("--device требует имя устройства")?,
            "--language" => language = it.next().ok_or("--language требует код языка")?,
            "--threads" => {
                threads = it
                    .next()
                    .ok_or("--threads требует число")?
                    .parse()
                    .map_err(|_| "--threads: ожидалось число")?
            }
            "--timecodes" => timecodes = true,
            "--no-trim" => trim = false,
            other if other.starts_with("--") => return Err(format!("неизвестный флаг {other}")),
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

/// Записать заданное число секунд с микрофона — сквозная проверка `CpalSource`.
fn record_from_mic(device: &str, secs: u32) -> Result<PcmAudio, String> {
    let (level_tx, level_rx) = std::sync::mpsc::channel();
    let mut source = CpalSource::new(device, 1.0, secs.max(1));

    source.start(Some(level_tx)).map_err(|e| e.to_string())?;
    eprintln!("говорите… запись {secs} с");
    std::thread::sleep(std::time::Duration::from_secs(secs.max(1) as u64));
    let audio = source.stop().map_err(|e| e.to_string())?;

    let levels: Vec<f32> = level_rx.try_iter().collect();
    let peak = levels.iter().copied().fold(0.0_f32, f32::max);
    eprintln!(
        "записано {:.2} с при {} Гц, замеров уровня {}, пиковый RMS {peak:.4}",
        audio.duration_secs(),
        audio.sample_rate,
        levels.len()
    );
    Ok(audio)
}

/// Модель по умолчанию — `small` в каталоге данных пользователя.
fn default_model_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("molva/models/ggml-small.bin")
}

/// Прочитать WAV любой разрядности и свести в моно.
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
