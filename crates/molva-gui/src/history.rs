// SPDX-License-Identifier: MIT
//! Чтение и правка журнала реплик (JSONL) для вкладки «История».
//!
//! Временная реализация: дорожка D даёт `molva_core::app::journal::FileJournal`
//! с `open/load_all/delete/clear`; тогда тело функций заменяется вызовами ядра,
//! а фильтрация (чистая функция ниже) остаётся здесь.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use molva_core::domain::Entry;
use molva_core::Config;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("не удалось прочитать журнал {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("не удалось записать журнал {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("не удалось определить каталог данных пользователя")]
    NoDataDir,
    #[error("не удалось сериализовать запись журнала: {0}")]
    Serialize(String),
}

/// Каталог данных: `$XDG_DATA_HOME/molva` и аналоги.
pub fn data_dir() -> Result<PathBuf, HistoryError> {
    directories::ProjectDirs::from("", "", "molva")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or(HistoryError::NoDataDir)
}

/// Путь к журналу: из конфига, а если там пусто — `<каталог данных>/journal.jsonl`.
pub fn journal_path(config: &Config) -> Result<PathBuf, HistoryError> {
    if !config.journal.path.trim().is_empty() {
        return Ok(PathBuf::from(&config.journal.path));
    }
    Ok(data_dir()?.join("journal.jsonl"))
}

/// Разбор журнала: битые строки пропускаются, их число возвращается вторым значением.
///
/// Журнал пишется дописыванием, поэтому оборванная последняя строка — норма, а не авария.
pub fn parse_lines(text: &str) -> (Vec<Entry>, usize) {
    let mut entries = Vec::new();
    let mut broken = 0usize;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Entry>(line) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                broken += 1;
                tracing::warn!(line = number + 1, %err, "пропущена битая строка журнала");
            }
        }
    }
    (entries, broken)
}

/// Все записи журнала, новые сверху. Отсутствующий файл — пустая история.
pub fn load_all(path: &Path) -> Result<Vec<Entry>, HistoryError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(HistoryError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let (mut entries, _) = parse_lines(&text);
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    Ok(entries)
}

/// Перезаписать журнал атомарно: временный файл рядом плюс переименование.
fn write_atomic(path: &Path, entries: &[Entry]) -> Result<(), HistoryError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| HistoryError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    let mut body = String::new();
    // Файл дописывается по времени, поэтому на диск возвращаем хронологический порядок.
    let mut ordered: Vec<&Entry> = entries.iter().collect();
    ordered.sort_by_key(|entry| entry.ts);
    for entry in ordered {
        let line =
            serde_json::to_string(entry).map_err(|e| HistoryError::Serialize(e.to_string()))?;
        body.push_str(&line);
        body.push('\n');
    }
    let temp = path.with_extension("jsonl.tmp");
    std::fs::write(&temp, body).map_err(|source| HistoryError::Write {
        path: temp.clone(),
        source,
    })?;
    std::fs::rename(&temp, path).map_err(|source| HistoryError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Удалить одну реплику. `false` — записи с таким идентификатором не было.
pub fn delete(path: &Path, id: Uuid) -> Result<bool, HistoryError> {
    let entries = load_all(path)?;
    let before = entries.len();
    let kept: Vec<Entry> = entries.into_iter().filter(|e| e.id != id).collect();
    if kept.len() == before {
        return Ok(false);
    }
    write_atomic(path, &kept)?;
    Ok(true)
}

/// Очистить журнал целиком.
pub fn clear(path: &Path) -> Result<(), HistoryError> {
    write_atomic(path, &[])
}

/// Условия отбора для вкладки «История». Пустые поля ничего не ограничивают.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Подстрока, без учёта регистра; ищется в исходном и итоговом тексте.
    pub query: String,
    /// Точное имя приложения из записи.
    pub app: String,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// 0 — без ограничения.
    pub limit: usize,
}

fn matches_query(entry: &Entry, needle: &str) -> bool {
    let hay = [entry.text_final.as_deref(), entry.text_raw.as_deref()];
    hay.iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(needle))
}

/// Отбор записей: подстрока, приложение, диапазон дат, ограничение по количеству.
pub fn filter_entries(entries: &[Entry], filter: &Filter) -> Vec<Entry> {
    let needle = filter.query.trim().to_lowercase();
    let app = filter.app.trim();
    let mut out: Vec<Entry> = entries
        .iter()
        .filter(|entry| needle.is_empty() || matches_query(entry, &needle))
        .filter(|entry| app.is_empty() || entry.app.as_deref() == Some(app))
        .filter(|entry| filter.since.is_none_or(|since| entry.ts >= since))
        .filter(|entry| filter.until.is_none_or(|until| entry.ts <= until))
        .cloned()
        .collect();
    if filter.limit > 0 {
        out.truncate(filter.limit);
    }
    out
}

/// Приложения, встречающиеся в журнале, по алфавиту — для выпадающего фильтра.
pub fn apps_of(entries: &[Entry]) -> Vec<String> {
    let mut apps: Vec<String> = entries.iter().filter_map(|e| e.app.clone()).collect();
    apps.sort();
    apps.dedup();
    apps
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::domain::entry::{LatencyMs, Mode, Source, SCHEMA_VERSION};

    fn entry(id: u128, ts: &str, app: Option<&str>, text: &str) -> Entry {
        Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::from_u128(id),
            ts: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::Mic,
            app: app.map(str::to_string),
            language: Some("ru".into()),
            audio_secs: 4.0,
            words: 10,
            wpm: Entry::wpm_for(10, 4.0),
            style: "cleanup".into(),
            stt_engine: "whisper-cpp".into(),
            stt_model: "small".into(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method: None,
            latency_ms: LatencyMs::default(),
            tokens: None,
            error: None,
            text_raw: Some(text.to_string()),
            text_final: Some(text.to_string()),
            audio_path: None,
        }
    }

    fn sample() -> Vec<Entry> {
        vec![
            entry(1, "2026-09-01T10:00:00Z", Some("kitty"), "Привет, мир"),
            entry(2, "2026-09-03T10:00:00Z", Some("firefox"), "Письмо коллеге"),
            entry(3, "2026-09-05T10:00:00Z", None, "ПРИВЕТ ещё раз"),
        ]
    }

    #[test]
    fn broken_lines_are_skipped_and_counted() {
        let good = serde_json::to_string(&entry(1, "2026-09-01T10:00:00Z", None, "раз")).unwrap();
        let text = format!("{good}\n{{битая строка\n\n{good}\n");
        let (entries, broken) = parse_lines(&text);
        assert_eq!(entries.len(), 2);
        assert_eq!(broken, 1);
    }

    #[test]
    fn empty_journal_parses_to_nothing() {
        assert_eq!(parse_lines("").0.len(), 0);
        assert_eq!(parse_lines("\n\n").1, 0);
    }

    #[test]
    fn search_is_case_insensitive_over_text() {
        let filter = Filter {
            query: "привет".into(),
            ..Default::default()
        };
        let found = filter_entries(&sample(), &filter);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn app_filter_is_exact_and_skips_entries_without_app() {
        let filter = Filter {
            app: "kitty".into(),
            ..Default::default()
        };
        let found = filter_entries(&sample(), &filter);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, Uuid::from_u128(1));
    }

    #[test]
    fn date_range_includes_boundaries() {
        let since = DateTime::parse_from_rfc3339("2026-09-03T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let filter = Filter {
            since: Some(since),
            ..Default::default()
        };
        let found = filter_entries(&sample(), &filter);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn limit_truncates_result() {
        let filter = Filter {
            limit: 2,
            ..Default::default()
        };
        assert_eq!(filter_entries(&sample(), &filter).len(), 2);
    }

    #[test]
    fn apps_are_unique_and_sorted() {
        assert_eq!(apps_of(&sample()), vec!["firefox", "kitty"]);
    }

    #[test]
    fn missing_journal_is_empty_history_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        assert!(load_all(&path).unwrap().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn newest_entries_come_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        write_atomic(&path, &sample()).unwrap();
        let loaded = load_all(&path).unwrap();
        assert_eq!(loaded[0].id, Uuid::from_u128(3));
        assert_eq!(loaded[2].id, Uuid::from_u128(1));
    }

    #[test]
    fn delete_removes_only_the_named_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        write_atomic(&path, &sample()).unwrap();
        assert!(delete(&path, Uuid::from_u128(2)).unwrap());
        let left = load_all(&path).unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|e| e.id != Uuid::from_u128(2)));
    }

    #[test]
    fn deleting_unknown_id_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        write_atomic(&path, &sample()).unwrap();
        assert!(!delete(&path, Uuid::from_u128(99)).unwrap());
        assert_eq!(load_all(&path).unwrap().len(), 3);
    }

    #[test]
    fn rewrite_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        write_atomic(&path, &sample()).unwrap();
        delete(&path, Uuid::from_u128(1)).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "остались временные файлы: {leftovers:?}"
        );
    }

    #[test]
    fn clear_empties_the_file_but_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        write_atomic(&path, &sample()).unwrap();
        clear(&path).unwrap();
        assert!(path.exists());
        assert!(load_all(&path).unwrap().is_empty());
    }

    #[test]
    fn journal_path_prefers_config_value() {
        let mut config = Config::default();
        config.journal.path = "/tmp/custom.jsonl".into();
        assert_eq!(
            journal_path(&config).unwrap(),
            PathBuf::from("/tmp/custom.jsonl")
        );
    }
}
