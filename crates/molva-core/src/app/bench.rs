// SPDX-License-Identifier: MIT
//! Local checker: runs a set of audio files through the engine and counts WER/CER and latency
//!
//! The organizers give no checker of their own, so quality is measured here with one command:
//! `molva bench`. A set is a directory with a `manifest.toml` that lists pairs of
//! "audio file → reference text". The score (`wer_avg`, `cer_avg`) depends only on the input
//! and the engine, so two runs on the same data give the same number (U-08). Latencies vary,
//! of course, and are not part of the score

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::app::wer::{cer, wer};
use crate::domain::stt::{LanguageHint, SttEngine, SttOptions};
use crate::infra::audio::decode;

/// Name of the file with the set description inside the directory
pub const MANIFEST_NAME: &str = "manifest.toml";

#[derive(Debug, Error)]
pub enum BenchError {
    #[error("check set not found: {0}. Expected a directory with manifest.toml")]
    SetMissing(PathBuf),
    #[error("error in {path}: {message}")]
    Manifest { path: PathBuf, message: String },
    #[error("the set {0} has no cases")]
    Empty(PathBuf),
}

/// One case of the set
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Case {
    /// Path to the audio, relative to the set directory
    pub audio: String,
    /// Reference text; an empty string means "silence expected"
    #[serde(default)]
    pub reference: String,
    /// Language of the case; `None` means auto-detection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Human-readable name of the case; the file name by default
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub case: Vec<Case>,
}

impl Manifest {
    pub fn from_toml_str(path: &Path, text: &str) -> Result<Self, BenchError> {
        toml::from_str(text).map_err(|e| BenchError::Manifest {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Read `manifest.toml` from the set directory
    pub fn load(set_dir: &Path) -> Result<Self, BenchError> {
        let path = set_dir.join(MANIFEST_NAME);
        let text = std::fs::read_to_string(&path)
            .map_err(|_| BenchError::SetMissing(set_dir.to_path_buf()))?;
        let manifest = Self::from_toml_str(&path, &text)?;
        if manifest.case.is_empty() {
            return Err(BenchError::Empty(set_dir.to_path_buf()));
        }
        Ok(manifest)
    }
}

/// Parameters of a run
#[derive(Debug, Clone, PartialEq)]
pub struct BenchOptions {
    /// How many times to run each case; more than one shows the spread of latencies
    pub repeat: usize,
    /// Number of engine threads; 0 means all cores
    pub threads: usize,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            repeat: 1,
            threads: 0,
        }
    }
}

/// Result of one case
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub audio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub audio_secs: f32,
    pub reference: String,
    pub hypothesis: String,
    pub wer: f32,
    pub cer: f32,
    /// Latencies of all repeats, in ms
    pub latency_ms: Vec<u32>,
    /// Ratio of recognition time to audio duration (below 1 means faster than real time)
    pub rtf: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Summary of latencies over all runs of the set
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct LatencySummary {
    pub p50_ms: u32,
    pub p95_ms: u32,
    pub p99_ms: u32,
    pub max_ms: u32,
    /// p99/p50: how many times the worst reply is slower than the typical one
    pub jitter: f32,
    pub samples: usize,
}

/// Full report of a run
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BenchReport {
    pub set: String,
    pub engine: String,
    pub model: String,
    pub repeat: usize,
    pub cases: Vec<CaseResult>,
    /// Cases that could not be run (missing file, engine error)
    pub failed: usize,
    /// Average WER over the successful cases
    pub wer_avg: f32,
    pub cer_avg: f32,
    pub audio_secs_total: f32,
    pub latency: LatencySummary,
    pub rtf_avg: f32,
}

impl BenchReport {
    /// Score of a run: what must match between two runs on the same data
    pub fn score(&self) -> (f32, f32, usize) {
        (self.wer_avg, self.cer_avg, self.failed)
    }
}

/// Percentile by the nearest-rank method; an empty input gives 0
fn percentile(sorted: &[u32], p: f32) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p / 100.0 * sorted.len() as f32).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn summarize_latency(mut all: Vec<u32>) -> LatencySummary {
    all.sort_unstable();
    let p50 = percentile(&all, 50.0);
    let p99 = percentile(&all, 99.0);
    LatencySummary {
        p50_ms: p50,
        p95_ms: percentile(&all, 95.0),
        p99_ms: p99,
        max_ms: all.last().copied().unwrap_or(0),
        jitter: if p50 == 0 {
            0.0
        } else {
            p99 as f32 / p50 as f32
        },
        samples: all.len(),
    }
}

/// Run the set through the engine
pub fn run(
    set_dir: &Path,
    options: &BenchOptions,
    engine: &mut dyn SttEngine,
) -> Result<BenchReport, BenchError> {
    let manifest = Manifest::load(set_dir)?;
    let repeat = options.repeat.max(1);

    let mut cases = Vec::with_capacity(manifest.case.len());
    let mut all_latencies: Vec<u32> = Vec::new();
    let mut failed = 0usize;

    for case in &manifest.case {
        let id = case
            .id
            .clone()
            .unwrap_or_else(|| case.audio.trim().to_string());
        let path = set_dir.join(&case.audio);
        let mut result = CaseResult {
            id,
            audio: case.audio.clone(),
            language: case.language.clone(),
            audio_secs: 0.0,
            reference: case.reference.clone(),
            hypothesis: String::new(),
            wer: 0.0,
            cer: 0.0,
            latency_ms: Vec::new(),
            rtf: 0.0,
            error: None,
        };

        let audio = match decode::decode_file(&path) {
            Ok(audio) => audio,
            Err(e) => {
                result.error = Some(e.to_string());
                failed += 1;
                cases.push(result);
                continue;
            }
        };
        result.audio_secs = audio.duration_secs();
        let audio = audio.to_16k();

        let sst_opts = SttOptions {
            language: case
                .language
                .as_deref()
                .map_or(LanguageHint::Auto, LanguageHint::parse),
            threads: options.threads,
            ..SttOptions::default()
        };

        let mut failure = None;
        for _ in 0..repeat {
            let started = Instant::now();
            match engine.transcribe(&audio, &sst_opts) {
                Ok(transcript) => {
                    result.latency_ms.push(started.elapsed().as_millis() as u32);
                    result.hypothesis = transcript.text;
                }
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(message) = failure {
            result.error = Some(message);
            failed += 1;
            cases.push(result);
            continue;
        }

        result.wer = wer(&result.reference, &result.hypothesis);
        result.cer = cer(&result.reference, &result.hypothesis);
        let median = percentile(
            &{
                let mut l = result.latency_ms.clone();
                l.sort_unstable();
                l
            },
            50.0,
        );
        result.rtf = if result.audio_secs > 0.0 {
            median as f32 / 1000.0 / result.audio_secs
        } else {
            0.0
        };
        all_latencies.extend(result.latency_ms.iter().copied());
        cases.push(result);
    }

    let ok: Vec<&CaseResult> = cases.iter().filter(|c| c.error.is_none()).collect();
    let mean = |values: Vec<f32>| -> f32 {
        if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f32>() / values.len() as f32
        }
    };

    Ok(BenchReport {
        set: set_dir.display().to_string(),
        engine: engine.id().to_string(),
        model: engine.model_name().to_string(),
        repeat,
        wer_avg: mean(ok.iter().map(|c| c.wer).collect()),
        cer_avg: mean(ok.iter().map(|c| c.cer).collect()),
        audio_secs_total: ok.iter().map(|c| c.audio_secs).sum(),
        rtf_avg: mean(ok.iter().map(|c| c.rtf).collect()),
        latency: summarize_latency(all_latencies),
        failed,
        cases,
    })
}

/// Human-readable summary of a run (T-15): a table of cases and the totals
pub fn format_summary(report: &BenchReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let width = report
        .cases
        .iter()
        .map(|c| c.id.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 40);

    let _ = writeln!(
        out,
        "{:<width$}  {:>7}  {:>7}  {:>9}  {:>8}",
        "case",
        "WER",
        "CER",
        "audio, s",
        "p50, ms",
        width = width
    );
    for case in &report.cases {
        let id: String = case.id.chars().take(width).collect();
        if let Some(err) = &case.error {
            let _ = writeln!(out, "{id:<width$}  error: {err}");
            continue;
        }
        let mut latencies = case.latency_ms.clone();
        latencies.sort_unstable();
        let _ = writeln!(
            out,
            "{:<width$}  {:>6.1}%  {:>6.1}%  {:>9.2}  {:>8}",
            id,
            case.wer * 100.0,
            case.cer * 100.0,
            case.audio_secs,
            percentile(&latencies, 50.0),
            width = width
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "engine {} / model {} / repeats {}",
        report.engine, report.model, report.repeat
    );
    let _ = writeln!(
        out,
        "cases {} (failed {}), audio {:.2} s",
        report.cases.len(),
        report.failed,
        report.audio_secs_total
    );
    let _ = writeln!(
        out,
        "WER {:.1} %   CER {:.1} %   RTF {:.2}",
        report.wer_avg * 100.0,
        report.cer_avg * 100.0,
        report.rtf_avg
    );
    let _ = writeln!(
        out,
        "latency p50 {} ms, p95 {} ms, p99 {} ms, p99/p50 {:.2}",
        report.latency.p50_ms, report.latency.p95_ms, report.latency.p99_ms, report.latency.jitter
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::FakeStt;
    use crate::domain::stt::{SttError, Transcript};

    fn write_wav(path: &Path, secs: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(16_000.0 * secs) as usize {
            let v = ((i as f32 * 0.05).sin() * 8_000.0) as i16;
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// A set of two cases with the reference "привет мир"
    fn make_set() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        write_wav(&directory.path().join("a.wav"), 1.0);
        write_wav(&directory.path().join("b.wav"), 0.5);
        std::fs::write(
            directory.path().join(MANIFEST_NAME),
            "[[case]]\naudio = \"a.wav\"\nreference = \"привет мир\"\nlanguage = \"ru\"\n\
             \n[[case]]\naudio = \"b.wav\"\nreference = \"привет мир\"\n",
        )
        .unwrap();
        directory
    }

    #[test]
    fn perfect_hypothesis_gives_zero_wer() {
        let directory = make_set();
        let mut stt = FakeStt::returning("Привет, мир!");
        let report = run(directory.path(), &BenchOptions::default(), &mut stt).unwrap();
        assert_eq!(report.cases.len(), 2);
        assert_eq!(report.failed, 0);
        assert!(report.wer_avg.abs() < 1e-6, "{}", report.wer_avg);
        assert!(report.cer_avg.abs() < 1e-6);
        assert!(report.audio_secs_total > 1.4);
    }

    #[test]
    fn wrong_hypothesis_raises_wer_but_run_still_succeeds() {
        let directory = make_set();
        let mut stt = FakeStt::returning("совсем другое");
        let report = run(directory.path(), &BenchOptions::default(), &mut stt).unwrap();
        assert!(report.wer_avg > 0.9, "{}", report.wer_avg);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn repeated_runs_produce_the_same_score() {
        let directory = make_set();
        let first = run(
            directory.path(),
            &BenchOptions {
                repeat: 2,
                threads: 0,
            },
            &mut FakeStt::returning("привет мир"),
        )
        .unwrap();
        let second = run(
            directory.path(),
            &BenchOptions {
                repeat: 2,
                threads: 0,
            },
            &mut FakeStt::returning("привет мир"),
        )
        .unwrap();
        assert_eq!(first.score(), second.score());
        assert_eq!(first.cases.len(), second.cases.len());
        for (a, b) in first.cases.iter().zip(&second.cases) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.hypothesis, b.hypothesis);
            assert_eq!(a.wer, b.wer);
        }
    }

    #[test]
    fn repeat_collects_a_latency_per_run() {
        let directory = make_set();
        let report = run(
            directory.path(),
            &BenchOptions {
                repeat: 3,
                threads: 0,
            },
            &mut FakeStt::returning("привет мир"),
        )
        .unwrap();
        assert_eq!(report.repeat, 3);
        assert_eq!(report.latency.samples, 6);
        for case in &report.cases {
            assert_eq!(case.latency_ms.len(), 3);
        }
    }

    #[test]
    fn missing_audio_marks_case_failed_without_killing_the_run() {
        let directory = tempfile::tempdir().unwrap();
        write_wav(&directory.path().join("a.wav"), 0.5);
        std::fs::write(
            directory.path().join(MANIFEST_NAME),
            "[[case]]\naudio = \"a.wav\"\nreference = \"привет\"\n\
             \n[[case]]\naudio = \"no-such-file.wav\"\nreference = \"привет\"\n",
        )
        .unwrap();
        let report = run(
            directory.path(),
            &BenchOptions::default(),
            &mut FakeStt::returning("привет"),
        )
        .unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.cases.len(), 2);
        assert!(report.cases[1].error.is_some());
        // The score counts the cases that passed, and a failed case does not reset it
        assert!(report.wer_avg.abs() < 1e-6);
    }

    #[test]
    fn engine_failure_is_recorded_per_case() {
        let directory = make_set();
        let mut stt = FakeStt::with_responses(vec![Err(SttError::Inference("broke".into()))]);
        let report = run(directory.path(), &BenchOptions::default(), &mut stt).unwrap();
        assert_eq!(report.failed, 2);
        assert!(report.cases[0].error.as_deref().unwrap().contains("broke"));
    }

    #[test]
    fn missing_set_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let err = run(
            &directory.path().join("missing"),
            &BenchOptions::default(),
            &mut FakeStt::returning("x"),
        )
        .unwrap_err();
        assert!(matches!(err, BenchError::SetMissing(_)), "{err}");
    }

    #[test]
    fn empty_manifest_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(MANIFEST_NAME), "# empty\n").unwrap();
        let err = Manifest::load(directory.path()).unwrap_err();
        assert!(matches!(err, BenchError::Empty(_)), "{err}");
    }

    #[test]
    fn broken_manifest_reports_path() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(MANIFEST_NAME),
            "[[case]]\naudio = 12\n",
        )
        .unwrap();
        let err = Manifest::load(directory.path()).unwrap_err();
        assert!(err.to_string().contains(MANIFEST_NAME), "{err}");
    }

    #[test]
    fn case_id_defaults_to_file_name_and_language_reaches_the_engine() {
        let directory = make_set();
        let mut stt = FakeStt::returning("привет мир");
        let report = run(directory.path(), &BenchOptions::default(), &mut stt).unwrap();
        assert_eq!(report.cases[0].id, "a.wav");
        assert_eq!(stt.calls[0].language, LanguageHint::Fixed("ru".into()));
        assert_eq!(stt.calls[1].language, LanguageHint::Auto);
    }

    #[test]
    fn summary_mentions_every_case_and_the_totals() {
        let directory = make_set();
        let report = run(
            directory.path(),
            &BenchOptions::default(),
            &mut FakeStt::returning("привет мир"),
        )
        .unwrap();
        let text = format_summary(&report);
        assert!(text.contains("a.wav"), "{text}");
        assert!(text.contains("b.wav"), "{text}");
        assert!(text.contains("WER"), "{text}");
        assert!(text.contains("p99"), "{text}");
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let sorted = vec![10u32, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(percentile(&sorted, 50.0), 50);
        assert_eq!(percentile(&sorted, 95.0), 100);
        assert_eq!(percentile(&sorted, 99.0), 100);
        assert_eq!(percentile(&[], 50.0), 0);
        assert_eq!(percentile(&[7], 99.0), 7);
    }

    #[test]
    fn latency_summary_reports_jitter_as_p99_over_p50() {
        let summary = summarize_latency(vec![100; 99].into_iter().chain([1000]).collect());
        assert_eq!(summary.p50_ms, 100);
        assert_eq!(summary.p99_ms, 100);
        assert_eq!(summary.max_ms, 1000);
        assert!((summary.jitter - 1.0).abs() < 1e-6);
    }

    #[test]
    fn report_serializes_to_json_with_cases() {
        let directory = make_set();
        let report = run(
            directory.path(),
            &BenchOptions::default(),
            &mut FakeStt::returning("привет мир"),
        )
        .unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"wer_avg\""), "{json}");
        assert!(json.contains("\"a.wav\""), "{json}");
        assert!(json.contains("\"p99_ms\""), "{json}");
    }

    #[test]
    fn transcript_segments_do_not_affect_the_score() {
        let directory = make_set();
        let mut stt = FakeStt::with_responses(vec![Ok(Transcript::text_only("привет мир"))]);
        let report = run(directory.path(), &BenchOptions::default(), &mut stt).unwrap();
        assert!(report.wer_avg.abs() < 1e-6);
    }
}
