// SPDX-License-Identifier: MIT
//! `molva bench` — the local checker of quality and speed
//!
//! One command runs the whole set and prints a summary: WER/CER per case and on average,
//! the latencies p50/p95/p99 and the ratio p99/p50. A bad WER is not a start error: it is a
//! report, so the exit code is 0. The only error is a missing set

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use molva_core::app::bench::{self, BenchOptions};
use molva_core::app::bench_latency::MemoryProbe;
use molva_core::app::engine::{build_stt_with, EngineChoice};
use molva_core::app::pipeline::{Pipeline, PipelineConfig};
use molva_core::domain::entry::Source;
use molva_core::domain::fakes::{MemJournal, RecordingInjector};
use molva_core::domain::stt::{LanguageHint, SttOptions};
use molva_core::domain::SystemClock;
use molva_core::infra::audio::decode;
use molva_core::ipc::protocol::FileTranscriptionRequest;
use molva_core::Config;

use super::CmdError;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Directory of the set with manifest.toml
    #[arg(long, value_name = "DIR", default_value = "bench")]
    pub set: PathBuf,

    /// Machine output: the full report in JSON
    #[arg(long)]
    pub json: bool,

    /// Print the summary table (the default behavior)
    #[arg(long)]
    pub summary: bool,

    /// How many times to run each case
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub repeat: usize,

    /// Recognition engine; `fake` checks the pipeline itself without weights
    #[arg(long, value_name = "NAME", env = "MOLVA_STT")]
    pub engine: Option<String>,

    /// Recognition model
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,

    /// Measure one audio file: recognition, post-processing and peak memory
    #[arg(long)]
    pub latency: bool,

    /// Audio for `--latency`; the default is the checked-in 15 s fixture
    #[arg(long, value_name = "FILE", default_value = "bench/latency-15s.wav")]
    pub audio: PathBuf,

    /// Do not run the post-processing stage in `--latency`
    #[arg(long)]
    pub no_postprocess: bool,

    /// Post-processing style for `--latency`; the default style of the settings when absent
    #[arg(long, value_name = "NAME")]
    pub style: Option<String>,
}

pub(crate) fn run(args: &Args, config: &Config, stdout: &mut dyn Write) -> Result<(), CmdError> {
    if args.repeat == 0 {
        return Err(CmdError::args("--repeat must be at least 1"));
    }
    if args.latency {
        return latency(args, config, stdout);
    }
    let choice = EngineChoice {
        engine: args.engine.clone(),
        model: args.model.clone(),
        // The fake engine answers with a fixed string and not with the reference of the case.
        // So the metric is computed and is not fitted to the answer
        fake_text: None,
    };
    let mut engine =
        build_stt_with(config, &choice).map_err(|e| CmdError::engine(e.to_string()))?;

    let options = BenchOptions {
        repeat: args.repeat,
        threads: config.stt.threads as usize,
    };
    let report = bench::run(&args.set, &options, engine.as_mut()).map_err(|e| match e {
        bench::BenchError::SetMissing(_) | bench::BenchError::Empty(_) => {
            CmdError::file(e.to_string())
        }
        bench::BenchError::Manifest { .. } => CmdError::args(e.to_string()),
    })?;

    let text = if args.json {
        serde_json::to_string_pretty(&report).map_err(|e| CmdError::file(e.to_string()))?
    } else {
        bench::format_summary(&report).trim_end().to_string()
    };
    writeln!(stdout, "{text}").map_err(|e| CmdError::file(e.to_string()))?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct LatencyReport {
    audio: String,
    audio_secs: f32,
    engine: String,
    model: String,
    repeat: usize,
    recognition_ms: Vec<u32>,
    postprocess_ms: Vec<u32>,
    peak_rss_mib: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    peak_vram_mib: Option<u64>,
    text: String,
}

fn latency(args: &Args, config: &Config, stdout: &mut dyn Write) -> Result<(), CmdError> {
    let audio = decode::decode_file(&args.audio).map_err(|e| CmdError::file(e.to_string()))?;
    let audio_secs = audio.duration_secs();
    let ready = audio.to_16k();
    let choice = EngineChoice {
        engine: args.engine.clone(),
        model: args.model.clone(),
        fake_text: None,
    };
    let mut engine =
        build_stt_with(config, &choice).map_err(|e| CmdError::engine(e.to_string()))?;
    let language = LanguageHint::parse(&config.stt.language);
    let options = SttOptions {
        language: language.clone(),
        threads: config.stt.threads as usize,
        ..SttOptions::default()
    };

    let probe = MemoryProbe::start();
    let mut recognition_ms = Vec::with_capacity(args.repeat);
    let mut text = String::new();
    for _ in 0..args.repeat {
        let started = std::time::Instant::now();
        let transcript = engine
            .transcribe(&ready, &options)
            .map_err(|e| CmdError::engine(e.to_string()))?;
        recognition_ms.push(started.elapsed().as_millis() as u32);
        text = transcript.text;
    }

    let mut postprocess_ms = Vec::new();
    if !args.no_postprocess {
        let mut pipeline = Pipeline::new(
            build_stt_with(config, &choice).map_err(|e| CmdError::engine(e.to_string()))?,
            super::daemon::build_llm(config),
            Box::new(RecordingInjector::default()),
            Box::new(MemJournal::default()),
            Arc::new(SystemClock),
            PipelineConfig::from_config(config),
        );
        let request = FileTranscriptionRequest {
            path: args.audio.clone(),
            style: args.style.clone(),
            timecodes: false,
            no_llm: false,
            source: Source::File,
        };
        for _ in 0..args.repeat {
            let started = std::time::Instant::now();
            let report = pipeline.transcribe_file(&request);
            let elapsed = started.elapsed().as_millis() as u32;
            let stt = recognition_ms
                .get(postprocess_ms.len())
                .copied()
                .unwrap_or_default();
            postprocess_ms.push(elapsed.saturating_sub(stt));
            if let Some(result) = report.results.first() {
                text.clone_from(&result.text);
            }
            if let Some(error) = report.errors.first() {
                return Err(CmdError::engine(error.reason.clone()));
            }
        }
    }
    let peak = probe.stop();

    let report = LatencyReport {
        audio: args.audio.display().to_string(),
        audio_secs,
        engine: engine.id().to_string(),
        model: engine.model_name().to_string(),
        repeat: args.repeat,
        recognition_ms,
        postprocess_ms,
        peak_rss_mib: peak.rss_bytes / (1024 * 1024),
        peak_vram_mib: peak.vram_bytes.map(|bytes| bytes / (1024 * 1024)),
        text,
    };
    if args.json {
        let text =
            serde_json::to_string_pretty(&report).map_err(|e| CmdError::file(e.to_string()))?;
        writeln!(stdout, "{text}").map_err(|e| CmdError::file(e.to_string()))?;
        return Ok(());
    }

    let median = |values: &[u32]| -> u32 {
        if values.is_empty() {
            return 0;
        }
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    };
    let recognition = median(&report.recognition_ms);
    let postprocess = median(&report.postprocess_ms);
    let vram = report
        .peak_vram_mib
        .map_or_else(|| "n/a".to_string(), |mib| format!("{mib} MiB"));
    writeln!(
        stdout,
        "{}  {:.2} s  {} / {}\nrecognition p50 {} ms  ({:.2}x realtime)\npost-processing p50 {} ms  (total {} ms)\npeak RAM {} MiB  peak VRAM {}\ntext: {}",
        report.audio,
        report.audio_secs,
        report.engine,
        report.model,
        recognition,
        if recognition == 0 { 0.0 } else { report.audio_secs * 1000.0 / recognition as f32 },
        postprocess,
        recognition.saturating_add(postprocess),
        report.peak_rss_mib,
        vram,
        report.text
    )
    .map_err(|e| CmdError::file(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_wav(path: &Path, secs: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(16_000.0 * secs) as usize {
            writer
                .write_sample(((i as f32 * 0.05).sin() * 8_000.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    fn make_set() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        write_wav(&directory.path().join("a.wav"), 0.4);
        std::fs::write(
            directory.path().join("manifest.toml"),
            "[[case]]\naudio = \"a.wav\"\nreference = \"тестовая расшифровка\"\nlanguage = \"ru\"\n",
        )
        .unwrap();
        directory
    }

    fn args_for(directory: &Path, json: bool) -> Args {
        Args {
            set: directory.to_path_buf(),
            json,
            summary: false,
            repeat: 1,
            engine: Some("fake".into()),
            model: None,
            latency: false,
            audio: PathBuf::from("bench/latency-15s.wav"),
            no_postprocess: false,
            style: None,
        }
    }

    #[test]
    fn fake_engine_runs_the_set_and_prints_a_summary() {
        let directory = make_set();
        let mut out = Vec::new();
        run(
            &args_for(directory.path(), false),
            &Config::default(),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("a.wav"), "{text}");
        assert!(text.contains("WER"), "{text}");
        assert!(text.contains("p99"), "{text}");
    }

    #[test]
    fn json_report_is_machine_readable() {
        let directory = make_set();
        let mut out = Vec::new();
        run(
            &args_for(directory.path(), true),
            &Config::default(),
            &mut out,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(value["wer_avg"].is_number());
        assert_eq!(value["cases"].as_array().unwrap().len(), 1);
        assert!(value["latency"]["p95_ms"].is_number());
    }

    #[test]
    fn two_runs_report_the_same_score() {
        let directory = make_set();
        let score = |json: &serde_json::Value| {
            (
                json["wer_avg"].as_f64().unwrap(),
                json["cer_avg"].as_f64().unwrap(),
                json["failed"].as_u64().unwrap(),
            )
        };
        let mut first = Vec::new();
        run(
            &args_for(directory.path(), true),
            &Config::default(),
            &mut first,
        )
        .unwrap();
        let mut second = Vec::new();
        run(
            &args_for(directory.path(), true),
            &Config::default(),
            &mut second,
        )
        .unwrap();
        let a: serde_json::Value = serde_json::from_slice(&first).unwrap();
        let b: serde_json::Value = serde_json::from_slice(&second).unwrap();
        assert_eq!(score(&a), score(&b));
    }

    #[test]
    fn missing_set_is_a_file_error() {
        let directory = tempfile::tempdir().unwrap();
        let error = run(
            &args_for(&directory.path().join("missing"), false),
            &Config::default(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code, CmdError::FILE);
    }

    #[test]
    fn bad_wer_is_still_a_successful_run() {
        let directory = tempfile::tempdir().unwrap();
        write_wav(&directory.path().join("a.wav"), 0.3);
        std::fs::write(
            directory.path().join("manifest.toml"),
            "[[case]]\naudio = \"a.wav\"\nreference = \"совершенно другой текст\"\n",
        )
        .unwrap();
        let mut out = Vec::new();
        run(
            &args_for(directory.path(), true),
            &Config::default(),
            &mut out,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(value["wer_avg"].as_f64().unwrap() > 0.5);
    }

    #[test]
    fn zero_repeat_is_an_argument_error() {
        let directory = make_set();
        let mut args = args_for(directory.path(), false);
        args.repeat = 0;
        let error = run(&args, &Config::default(), &mut Vec::new()).unwrap_err();
        assert_eq!(error.code, CmdError::BAD_ARGS);
    }

    #[test]
    fn repeat_is_reflected_in_the_report() {
        let directory = make_set();
        let mut args = args_for(directory.path(), true);
        args.repeat = 3;
        let mut out = Vec::new();
        run(&args, &Config::default(), &mut out).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["repeat"], 3);
        assert_eq!(value["latency"]["samples"], 3);
    }
}
