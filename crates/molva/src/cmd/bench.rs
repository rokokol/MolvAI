// SPDX-License-Identifier: MIT
//! `molva bench` — собственный чекер качества и скорости.
//!
//! Одна команда прогоняет весь набор и печатает сводку: WER/CER по кейсам и в среднем,
//! задержки p50/p95/p99 и отношение p99/p50. Плохой WER — не ошибка запуска: это отчёт,
//! поэтому код выхода 0. Ошибка — только отсутствующий набор.

use std::io::Write;
use std::path::PathBuf;

use molva_core::app::bench::{self, BenchOptions};
use molva_core::app::engine::{build_stt_with, EngineChoice};
use molva_core::Config;

use super::CmdError;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Каталог набора с manifest.toml
    #[arg(long, value_name = "DIR", default_value = "bench")]
    pub set: PathBuf,

    /// Машинный вывод: полный отчёт в JSON
    #[arg(long)]
    pub json: bool,

    /// Печатать сводную таблицу (поведение по умолчанию)
    #[arg(long)]
    pub summary: bool,

    /// Сколько раз прогнать каждый кейс
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub repeat: usize,

    /// Движок распознавания; `fake` проверяет сам конвейер без весов
    #[arg(long, value_name = "NAME", env = "MOLVA_STT")]
    pub engine: Option<String>,

    /// Модель распознавания
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,
}

pub(crate) fn run(args: &Args, config: &Config, stdout: &mut dyn Write) -> Result<(), CmdError> {
    if args.repeat == 0 {
        return Err(CmdError::args("--repeat должен быть не меньше 1"));
    }
    let choice = EngineChoice {
        engine: args.engine.clone(),
        model: args.model.clone(),
        // Фейковый движок в бенче отвечает эталоном первого кейса? Нет: он отвечает
        // фиксированной строкой, и это осознанно — так видно, что метрика реально считается,
        // а не подгоняется под ответ.
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
            &args_for(&directory.path().join("нет"), false),
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
