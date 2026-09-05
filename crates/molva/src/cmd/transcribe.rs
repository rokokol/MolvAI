// SPDX-License-Identifier: MIT
//! `molva transcribe` — расшифровка файлов, каталогов и stdin.
//!
//! Это офлайн-путь: он ничего никуда не вставляет и не трогает микрофон, поэтому его можно
//! гонять в скриптах и в CI. Ошибка одного файла в пакете не отменяет остальные — итог
//! получают все, кто смог, а код выхода 6 сообщает, что не всё прошло.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use molva_core::app::engine::{build_stt_with, EngineChoice};
use molva_core::domain::entry::{Entry, LatencyMs, Mode, Source, SCHEMA_VERSION};
use molva_core::domain::journal::Journal;
use molva_core::domain::stt::{LanguageHint, Segment, SttEngine, SttOptions};
use molva_core::domain::text::word_count;
use molva_core::infra::audio::decode;
use molva_core::Config;
use serde::Serialize;
use uuid::Uuid;

use super::{progress_enabled, CmdError};

/// Постобработка распознанного текста: словарь, правила и модель подключаются конвейером
/// дорожки D. Пока по умолчанию — тождественная функция, точка вызова уже на месте.
pub fn identity(text: &str) -> String {
    text.to_string()
}

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Аудиофайлы, каталоги или `-` для чтения потока со стандартного ввода
    #[arg(value_name = "PATH", required = true)]
    pub input: Vec<PathBuf>,

    /// Язык записи (код ISO-639-1); по умолчанию берётся из настроек
    #[arg(long, value_name = "CODE")]
    pub language: Option<String>,

    /// Модель распознавания: tiny, base, small, large-v3-turbo…
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,

    /// Движок распознавания; `fake` прогоняет конвейер без весов
    #[arg(long, value_name = "NAME", env = "MOLVA_STT")]
    pub engine: Option<String>,

    /// Стиль постобработки; применяется конвейером обработки текста
    #[arg(long, value_name = "ID")]
    pub style: Option<String>,

    /// Не обращаться к языковой модели при постобработке
    #[arg(long)]
    pub no_llm: bool,

    /// Принимается для совместимости с диктовкой: `transcribe` никогда никуда не вставляет текст
    #[arg(long)]
    pub no_inject: bool,

    /// Машинный вывод: массив JSON вместо текста
    #[arg(long)]
    pub json: bool,

    /// Печатать сегменты с таймкодами
    #[arg(long)]
    pub timecodes: bool,

    /// Файл или каталог для результата; без него текст идёт в stdout
    #[arg(short = 'o', long, value_name = "PATH")]
    pub out: Option<PathBuf>,

    /// Обходить вложенные каталоги
    #[arg(long)]
    pub recursive: bool,

    /// Подсказка формата для потока на stdin: wav, mp3, ogg, flac, m4a
    #[arg(long, value_name = "FORMAT")]
    pub stdin_format: Option<String>,
}

/// Что расшифровываем: файл на диске или поток на входе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobSource {
    File(PathBuf),
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Имя для вывода и журнала.
    pub label: String,
    pub source: JobSource,
}

/// Результат по одному входу.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileResult {
    pub file: String,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,
    pub audio_secs: f32,
    pub latency_ms: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Итог пакета: что получилось и что нет.
#[derive(Debug, Default)]
pub struct Outcome {
    pub results: Vec<FileResult>,
    /// Пары «вход → причина отказа».
    pub errors: Vec<(String, String)>,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Собрать список входов: файлы по порядку аргументов, содержимое каталогов — по алфавиту.
pub fn collect_jobs(inputs: &[PathBuf], recursive: bool) -> Result<Vec<Job>, CmdError> {
    let mut jobs = Vec::new();
    for input in inputs {
        if input.as_os_str() == "-" {
            jobs.push(Job {
                label: decode::STDIN_LABEL.to_string(),
                source: JobSource::Stdin,
            });
            continue;
        }
        let meta = std::fs::metadata(input)
            .map_err(|e| CmdError::file(format!("{}: {e}", input.display())))?;
        if meta.is_dir() {
            collect_dir(input, recursive, &mut jobs)?;
        } else {
            jobs.push(Job {
                label: input.display().to_string(),
                source: JobSource::File(input.clone()),
            });
        }
    }
    if jobs.is_empty() {
        return Err(CmdError::file(
            "не найдено ни одного аудиофайла: проверьте путь или добавьте --recursive",
        ));
    }
    Ok(jobs)
}

fn collect_dir(dir: &Path, recursive: bool, jobs: &mut Vec<Job>) -> Result<(), CmdError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| CmdError::file(format!("{}: {e}", dir.display())))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    // Порядок обхода каталога в файловой системе произволен; сортировка делает вывод
    // повторяемым от запуска к запуску.
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if recursive {
                collect_dir(&path, recursive, jobs)?;
            }
        } else if decode::is_supported_audio(&path) {
            jobs.push(Job {
                label: path.display().to_string(),
                source: JobSource::File(path),
            });
        }
    }
    Ok(())
}

/// Прогнать список входов через движок.
///
/// `postprocess` — точка подключения конвейера обработки текста; `progress` вызывается
/// перед каждым входом и пишет только в stderr.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_jobs(
    jobs: &[Job],
    engine: &mut dyn SttEngine,
    opts: &SttOptions,
    stdin_format: Option<&str>,
    postprocess: &dyn Fn(&str) -> String,
    journal: &mut dyn Journal,
    style: &str,
    progress: &mut dyn FnMut(usize, &str),
) -> Outcome {
    let mut outcome = Outcome::default();
    let engine_id = engine.id().to_string();
    let model_name = engine.model_name().to_string();
    let session_id = Uuid::new_v4();

    for (index, job) in jobs.iter().enumerate() {
        progress(index, &job.label);
        let decoded = match &job.source {
            JobSource::File(path) => decode::decode_file(path),
            JobSource::Stdin => decode::decode_stdin(stdin_format),
        };
        let audio = match decoded {
            Ok(audio) => audio,
            Err(e) => {
                outcome.errors.push((job.label.clone(), e.to_string()));
                continue;
            }
        };
        let audio_secs = audio.duration_secs();
        let ready = audio.to_16k();

        let started = Instant::now();
        let transcript = match engine.transcribe(&ready, opts) {
            Ok(transcript) => transcript,
            Err(e) => {
                outcome.errors.push((job.label.clone(), e.to_string()));
                continue;
            }
        };
        let stt_ms = started.elapsed().as_millis() as u32;

        let raw = transcript.text.clone();
        let text = postprocess(&raw);
        let total_ms = started.elapsed().as_millis() as u32;
        let words = word_count(&text) as u32;

        let entry = Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            ts: Utc::now(),
            session_id,
            mode: Mode::Dictation,
            // Источник — файл, а не микрофон: по этому полю статистика отделяет пакетную
            // расшифровку от живой диктовки.
            source: Source::File,
            app: None,
            language: transcript.detected_language.clone(),
            audio_secs,
            words,
            wpm: Entry::wpm_for(words, audio_secs),
            style: style.to_string(),
            stt_engine: engine_id.clone(),
            stt_model: model_name.clone(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method: None,
            latency_ms: LatencyMs {
                stt: stt_ms,
                rules: total_ms.saturating_sub(stt_ms),
                total: total_ms,
                ..LatencyMs::default()
            },
            tokens: None,
            error: None,
            text_raw: Some(raw),
            text_final: Some(text.clone()),
            audio_path: None,
        };
        if let Err(e) = journal.append(&entry) {
            // Журнал — вспомогательная функция: его отказ не должен стоить пользователю текста.
            eprintln!("предупреждение: запись в журнал не удалась: {e}");
        }

        outcome.results.push(FileResult {
            file: job.label.clone(),
            text,
            segments: transcript.segments,
            audio_secs,
            latency_ms: total_ms,
            language: transcript.detected_language,
        });
    }
    outcome
}

/// `12345` мс → `00:12.345`.
pub fn format_timecode(ms: u32) -> String {
    let total_secs = ms / 1000;
    format!(
        "{:02}:{:02}.{:03}",
        total_secs / 60,
        total_secs % 60,
        ms % 1000
    )
}

/// Текст одного результата: либо сплошняком, либо строками с таймкодами.
pub fn render_text(result: &FileResult, timecodes: bool) -> String {
    if !timecodes {
        return result.text.clone();
    }
    if result.segments.is_empty() {
        // Движок не отдал сегменты — показываем весь текст одним отрезком, а не пустоту.
        let end = (result.audio_secs * 1000.0) as u32;
        return format!(
            "[{} → {}] {}",
            format_timecode(0),
            format_timecode(end),
            result.text
        );
    }
    result
        .segments
        .iter()
        .map(|s| {
            format!(
                "[{} → {}] {}",
                format_timecode(s.start_ms),
                format_timecode(s.end_ms),
                s.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Собрать вывод для нескольких входов: перед каждым — заголовок с именем, если входов больше одного.
pub fn render_all(results: &[FileResult], timecodes: bool) -> String {
    if results.len() == 1 {
        return render_text(&results[0], timecodes);
    }
    results
        .iter()
        .map(|r| format!("=== {} ===\n{}", r.file, render_text(r, timecodes)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn out_file_for(dir: &Path, result: &FileResult, json: bool) -> PathBuf {
    let stem = Path::new(&result.file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "transcript".to_string());
    dir.join(format!("{stem}.{}", if json { "json" } else { "txt" }))
}

/// Записать результаты туда, куда просил пользователь.
pub fn write_output(
    outcome: &Outcome,
    args: &Args,
    stdout: &mut dyn Write,
) -> Result<(), CmdError> {
    let to_file = |path: &Path, body: &str| -> Result<(), CmdError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| CmdError::file(format!("{}: {e}", parent.display())))?;
        }
        std::fs::write(path, body).map_err(|e| CmdError::file(format!("{}: {e}", path.display())))
    };

    match &args.out {
        // Каталог: по файлу на вход, имя — от исходного файла.
        Some(path) if path.is_dir() => {
            for result in &outcome.results {
                let target = out_file_for(path, result, args.json);
                let body = if args.json {
                    serde_json::to_string_pretty(result)
                        .map_err(|e| CmdError::file(e.to_string()))?
                } else {
                    render_text(result, args.timecodes)
                };
                to_file(&target, &format!("{body}\n"))?;
            }
        }
        Some(path) => {
            let body = if args.json {
                serde_json::to_string_pretty(&outcome.results)
                    .map_err(|e| CmdError::file(e.to_string()))?
            } else {
                render_all(&outcome.results, args.timecodes)
            };
            to_file(path, &format!("{body}\n"))?;
        }
        None => {
            let body = if args.json {
                serde_json::to_string_pretty(&outcome.results)
                    .map_err(|e| CmdError::file(e.to_string()))?
            } else {
                render_all(&outcome.results, args.timecodes)
            };
            writeln!(stdout, "{body}").map_err(|e| CmdError::file(e.to_string()))?;
        }
    }
    Ok(())
}

/// Точка входа подкоманды.
pub fn run(args: &Args, cfg: &Config) -> Result<(), CmdError> {
    let jobs = collect_jobs(&args.input, args.recursive)?;

    let choice = EngineChoice {
        engine: args.engine.clone(),
        model: args.model.clone(),
        fake_text: None,
    };
    let mut engine = build_stt_with(cfg, &choice).map_err(|e| CmdError::engine(e.to_string()))?;

    let language = args.language.as_deref().unwrap_or(&cfg.stt.language);
    let opts = SttOptions {
        language: LanguageHint::parse(language),
        allowed_languages: cfg.stt.allowed_languages.clone(),
        initial_prompt: None,
        threads: cfg.stt.threads as usize,
        timestamps: args.timecodes,
    };

    let style = args
        .style
        .as_deref()
        .unwrap_or(&cfg.style.default)
        .to_string();
    // Файловый журнал подключает конвейер дорожки D; здесь запись собирается и уходит
    // в приёмник, который передали.
    let mut journal = molva_core::domain::fakes::MemJournal::default();

    let bar = (jobs.len() > 1 && progress_enabled(args.json))
        .then(|| indicatif::ProgressBar::new(jobs.len() as u64));
    if let Some(bar) = &bar {
        if let Ok(style) =
            indicatif::ProgressStyle::with_template("{spinner} [{pos}/{len}] {wide_msg}")
        {
            bar.set_style(style);
        }
    }

    let outcome = transcribe_jobs(
        &jobs,
        engine.as_mut(),
        &opts,
        args.stdin_format.as_deref(),
        &identity,
        &mut journal,
        &style,
        &mut |index, label| {
            if let Some(bar) = &bar {
                bar.set_position(index as u64);
                bar.set_message(label.to_string());
            }
        },
    );
    if let Some(bar) = bar {
        bar.finish_and_clear();
    }

    let mut stdout = std::io::stdout().lock();
    write_output(&outcome, args, &mut stdout)?;

    for (label, reason) in &outcome.errors {
        eprintln!("ошибка: {label}: {reason}");
    }
    if !outcome.ok() {
        return Err(CmdError::file(format!(
            "не удалось расшифровать файлов: {} из {}",
            outcome.errors.len(),
            jobs.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::domain::fakes::{FakeStt, MemJournal};
    use molva_core::domain::stt::Transcript;

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

    fn jobs_for(paths: &[PathBuf]) -> Vec<Job> {
        paths
            .iter()
            .map(|p| Job {
                label: p.display().to_string(),
                source: JobSource::File(p.clone()),
            })
            .collect()
    }

    fn run_fake(jobs: &[Job], text: &str) -> Outcome {
        let mut stt = FakeStt::returning(text);
        let mut journal = MemJournal::default();
        transcribe_jobs(
            jobs,
            &mut stt,
            &SttOptions::default(),
            None,
            &identity,
            &mut journal,
            "cleanup",
            &mut |_, _| {},
        )
    }

    #[test]
    fn single_file_is_transcribed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1.0);

        let outcome = run_fake(&jobs_for(std::slice::from_ref(&path)), "привет мир");
        assert!(outcome.ok());
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].text, "привет мир");
        assert!((outcome.results[0].audio_secs - 1.0).abs() < 0.01);
    }

    #[test]
    fn two_runs_on_the_same_file_give_the_same_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 0.5);
        let jobs = jobs_for(&[path]);

        let first = run_fake(&jobs, "привет мир");
        let second = run_fake(&jobs, "привет мир");
        assert_eq!(first.results[0].text, second.results[0].text);
        assert_eq!(first.results[0].audio_secs, second.results[0].audio_secs);
        assert_eq!(first.results[0].file, second.results[0].file);
    }

    #[test]
    fn broken_file_does_not_stop_the_batch() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.wav");
        write_wav(&good, 0.3);
        let bad = dir.path().join("bad.wav");
        std::fs::write(&bad, "не аудио").unwrap();

        let outcome = run_fake(&jobs_for(&[bad, good]), "текст");
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.errors.len(), 1);
        assert!(!outcome.ok());
        assert!(outcome.errors[0].0.ends_with("bad.wav"));
    }

    #[test]
    fn directory_is_walked_in_alphabetical_order() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["c.wav", "a.wav", "b.wav"] {
            write_wav(&dir.path().join(name), 0.1);
        }
        std::fs::write(dir.path().join("notes.txt"), "не аудио").unwrap();

        let jobs = collect_jobs(&[dir.path().to_path_buf()], false).unwrap();
        let names: Vec<String> = jobs
            .iter()
            .map(|j| {
                Path::new(&j.label)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["a.wav", "b.wav", "c.wav"]);
    }

    #[test]
    fn nested_directories_need_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("вложенный");
        std::fs::create_dir(&nested).unwrap();
        write_wav(&nested.join("a.wav"), 0.1);
        write_wav(&dir.path().join("b.wav"), 0.1);

        assert_eq!(
            collect_jobs(&[dir.path().to_path_buf()], false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            collect_jobs(&[dir.path().to_path_buf()], true)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn dash_means_stdin() {
        let jobs = collect_jobs(&[PathBuf::from("-")], false).unwrap();
        assert_eq!(jobs[0].source, JobSource::Stdin);
        assert_eq!(jobs[0].label, decode::STDIN_LABEL);
    }

    #[test]
    fn empty_directory_is_an_error_not_a_silent_success() {
        let dir = tempfile::tempdir().unwrap();
        let err = collect_jobs(&[dir.path().to_path_buf()], false).unwrap_err();
        assert_eq!(err.code, CmdError::FILE);
    }

    #[test]
    fn missing_input_is_reported_with_path() {
        let err = collect_jobs(&[PathBuf::from("/нет/такого.wav")], false).unwrap_err();
        assert!(err.message.contains("/нет/такого.wav"), "{}", err.message);
        assert_eq!(err.code, CmdError::FILE);
    }

    #[test]
    fn journal_records_the_file_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1.5);
        let mut stt = FakeStt::returning("привет мир друзья");
        let mut journal = MemJournal::default();

        transcribe_jobs(
            &jobs_for(&[path]),
            &mut stt,
            &SttOptions::default(),
            None,
            &identity,
            &mut journal,
            "cleanup",
            &mut |_, _| {},
        );

        assert_eq!(journal.entries.len(), 1);
        let entry = &journal.entries[0];
        assert_eq!(entry.source, Source::File);
        assert_eq!(entry.words, 3);
        assert_eq!(entry.stt_engine, "fake");
        assert_eq!(entry.text_final.as_deref(), Some("привет мир друзья"));
    }

    #[test]
    fn postprocess_hook_is_applied_to_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 0.2);
        let mut stt = FakeStt::returning("привет");
        let mut journal = MemJournal::default();

        let outcome = transcribe_jobs(
            &jobs_for(&[path]),
            &mut stt,
            &SttOptions::default(),
            None,
            &|text| format!("{text}!"),
            &mut journal,
            "cleanup",
            &mut |_, _| {},
        );
        assert_eq!(outcome.results[0].text, "привет!");
        // В журнал попадает и сырой текст, и итоговый: видно, что сделала постобработка.
        assert_eq!(journal.entries[0].text_raw.as_deref(), Some("привет"));
        assert_eq!(journal.entries[0].text_final.as_deref(), Some("привет!"));
    }

    #[test]
    fn timecode_format_is_minutes_seconds_millis() {
        assert_eq!(format_timecode(0), "00:00.000");
        assert_eq!(format_timecode(2_500), "00:02.500");
        assert_eq!(format_timecode(75_120), "01:15.120");
    }

    #[test]
    fn timecodes_render_segments() {
        let result = FileResult {
            file: "a.wav".into(),
            text: "привет мир".into(),
            segments: vec![
                Segment {
                    start_ms: 0,
                    end_ms: 2_500,
                    text: " привет".into(),
                },
                Segment {
                    start_ms: 2_500,
                    end_ms: 4_000,
                    text: "мир".into(),
                },
            ],
            audio_secs: 4.0,
            latency_ms: 10,
            language: Some("ru".into()),
        };
        let text = render_text(&result, true);
        assert_eq!(
            text,
            "[00:00.000 → 00:02.500] привет\n[00:02.500 → 00:04.000] мир"
        );
        assert_eq!(render_text(&result, false), "привет мир");
    }

    #[test]
    fn timecodes_without_segments_wrap_the_whole_text() {
        let result = FileResult {
            file: "a.wav".into(),
            text: "привет".into(),
            segments: vec![],
            audio_secs: 1.5,
            latency_ms: 1,
            language: None,
        };
        assert_eq!(render_text(&result, true), "[00:00.000 → 00:01.500] привет");
    }

    #[test]
    fn several_results_are_labelled_by_file() {
        let make = |name: &str| FileResult {
            file: name.into(),
            text: "текст".into(),
            segments: vec![],
            audio_secs: 1.0,
            latency_ms: 1,
            language: None,
        };
        let text = render_all(&[make("a.wav"), make("b.wav")], false);
        assert!(text.contains("=== a.wav ==="), "{text}");
        assert!(text.contains("=== b.wav ==="), "{text}");
    }

    #[test]
    fn json_output_has_the_documented_fields() {
        let result = FileResult {
            file: "a.wav".into(),
            text: "привет".into(),
            segments: vec![],
            audio_secs: 1.0,
            latency_ms: 7,
            language: Some("ru".into()),
        };
        let json = serde_json::to_string(&[result]).unwrap();
        for field in [
            "\"file\"",
            "\"text\"",
            "\"audio_secs\"",
            "\"latency_ms\"",
            "\"language\"",
        ] {
            assert!(json.contains(field), "нет {field} в {json}");
        }
        // Пустой список сегментов не засоряет вывод.
        assert!(!json.contains("\"segments\""), "{json}");
    }

    #[test]
    fn output_directory_gets_one_file_per_input() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let outcome = Outcome {
            results: vec![
                FileResult {
                    file: "/данные/первый.wav".into(),
                    text: "раз".into(),
                    segments: vec![],
                    audio_secs: 1.0,
                    latency_ms: 1,
                    language: None,
                },
                FileResult {
                    file: "/данные/второй.wav".into(),
                    text: "два".into(),
                    segments: vec![],
                    audio_secs: 1.0,
                    latency_ms: 1,
                    language: None,
                },
            ],
            errors: vec![],
        };
        let args = Args {
            input: vec![],
            language: None,
            model: None,
            engine: None,
            style: None,
            no_llm: false,
            no_inject: false,
            json: false,
            timecodes: false,
            out: Some(out.clone()),
            recursive: false,
            stdin_format: None,
        };
        write_output(&outcome, &args, &mut Vec::new()).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("первый.txt")).unwrap(),
            "раз\n"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("второй.txt")).unwrap(),
            "два\n"
        );
    }

    #[test]
    fn output_file_collects_everything_and_stdout_stays_empty() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("всё.txt");
        let outcome = Outcome {
            results: vec![FileResult {
                file: "a.wav".into(),
                text: "привет".into(),
                segments: vec![],
                audio_secs: 1.0,
                latency_ms: 1,
                language: None,
            }],
            errors: vec![],
        };
        let args = Args {
            input: vec![],
            language: None,
            model: None,
            engine: None,
            style: None,
            no_llm: false,
            no_inject: false,
            json: false,
            timecodes: false,
            out: Some(target.clone()),
            recursive: false,
            stdin_format: None,
        };
        let mut stdout = Vec::new();
        write_output(&outcome, &args, &mut stdout).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "привет\n");
        assert!(
            stdout.is_empty(),
            "данные должны уйти в файл, а не в stdout"
        );
    }

    #[test]
    fn without_out_the_text_goes_to_stdout() {
        let outcome = Outcome {
            results: vec![FileResult {
                file: "a.wav".into(),
                text: "привет".into(),
                segments: vec![],
                audio_secs: 1.0,
                latency_ms: 1,
                language: None,
            }],
            errors: vec![],
        };
        let args = Args {
            input: vec![],
            language: None,
            model: None,
            engine: None,
            style: None,
            no_llm: false,
            no_inject: false,
            json: false,
            timecodes: false,
            out: None,
            recursive: false,
            stdin_format: None,
        };
        let mut stdout = Vec::new();
        write_output(&outcome, &args, &mut stdout).unwrap();
        assert_eq!(String::from_utf8(stdout).unwrap(), "привет\n");
    }

    #[test]
    fn engine_error_is_recorded_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 0.2);
        let mut stt = FakeStt::with_responses(vec![Err(
            molva_core::domain::stt::SttError::Inference("сломалось".into()),
        )]);
        let mut journal = MemJournal::default();
        let outcome = transcribe_jobs(
            &jobs_for(&[path]),
            &mut stt,
            &SttOptions::default(),
            None,
            &identity,
            &mut journal,
            "cleanup",
            &mut |_, _| {},
        );
        assert!(outcome.results.is_empty());
        assert!(outcome.errors[0].1.contains("сломалось"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn segments_from_the_engine_reach_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 0.5);
        let mut stt = FakeStt::with_responses(vec![Ok(Transcript {
            text: "привет".into(),
            segments: vec![Segment {
                start_ms: 0,
                end_ms: 500,
                text: "привет".into(),
            }],
            detected_language: Some("ru".into()),
            no_speech_prob: None,
        })]);
        let mut journal = MemJournal::default();
        let outcome = transcribe_jobs(
            &jobs_for(&[path]),
            &mut stt,
            &SttOptions::default(),
            None,
            &identity,
            &mut journal,
            "cleanup",
            &mut |_, _| {},
        );
        assert_eq!(outcome.results[0].segments.len(), 1);
        assert_eq!(outcome.results[0].language.as_deref(), Some("ru"));
    }
}
