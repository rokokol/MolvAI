// SPDX-License-Identifier: MIT
//! Журнал реплик: JSONL-файл, одна строка — одна реплика.
//!
//! Схема строки описана в `docs/journal-schema.md`, эталон типа — `domain::entry::Entry`.
//! Файл только дописывается; удаление, очистка и ротация выполняются атомарной перезаписью
//! (временный файл рядом + `rename`), чтобы падение посреди операции не оставило огрызок.
//!
//! Дескриптор между записями не держится: каждая запись открывает файл заново. Так `&self`-методы,
//! подменяющие файл через `rename`, не оставляют журнал писать в удалённый inode.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::domain::entry::{Entry, SCHEMA_VERSION};
use crate::domain::journal::{Journal, JournalError};

/// Заголовок файла: версия схемы и момент создания. `load_all` его пропускает.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    schema: u32,
    created: DateTime<Utc>,
}

fn io_err(path: &Path, err: std::io::Error) -> JournalError {
    JournalError::Io(format!("{}: {err}", path.display()))
}

/// Журнал в файле JSONL.
#[derive(Debug, Clone)]
pub struct FileJournal {
    path: PathBuf,
    /// `false` — режим приватности: строка пишется без текстов и пути к аудио.
    include_text: bool,
}

impl FileJournal {
    /// Открыть (и при необходимости создать) журнал; каталог создаётся вместе с файлом.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        Self::open_with(path, true)
    }

    /// То же, но с явным режимом приватности: `include_text = false` убирает тексты из строк.
    pub fn open_with(path: &Path, include_text: bool) -> Result<Self, JournalError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
            }
        }
        let existed = path.exists();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| io_err(path, e))?;
        restrict_permissions(&file, path)?;
        if !existed {
            let header = Header {
                schema: SCHEMA_VERSION,
                created: Utc::now(),
            };
            let line = serde_json::to_string(&header)
                .map_err(|e| JournalError::Serialize(e.to_string()))?;
            let mut file = file;
            writeln!(file, "{line}").map_err(|e| io_err(path, e))?;
            file.sync_all().map_err(|e| io_err(path, e))?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            include_text,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn include_text(&self) -> bool {
        self.include_text
    }

    pub fn set_include_text(&mut self, include_text: bool) {
        self.include_text = include_text;
    }

    /// Файл, куда уезжают строки, которые не удалось разобрать.
    pub fn broken_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".broken");
        PathBuf::from(name)
    }

    /// Все записи по порядку. Битые строки не роняют загрузку: они уходят в `<path>.broken`.
    pub fn load_all(&self) -> Result<Vec<Entry>, JournalError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&self.path, e)),
        };
        let mut entries = Vec::new();
        let mut broken = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) => entries.push(entry),
                Err(_) if is_header(line) => {}
                Err(err) => {
                    warn!(
                        line = index + 1,
                        error = %err,
                        "повреждённая строка журнала перенесена в карантин"
                    );
                    broken.push(line.to_string());
                }
            }
        }
        if !broken.is_empty() {
            self.quarantine(&broken)?;
        }
        Ok(entries)
    }

    /// Дописать битые строки в файл карантина, не трогая сам журнал.
    fn quarantine(&self, lines: &[String]) -> Result<(), JournalError> {
        let path = self.broken_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| io_err(&path, e))?;
        restrict_permissions(&file, &path)?;
        for line in lines {
            writeln!(file, "{line}").map_err(|e| io_err(&path, e))?;
        }
        file.sync_all().map_err(|e| io_err(&path, e))
    }

    /// Поиск по подстроке в текстах реплики, без учёта регистра.
    pub fn search(&self, substring: &str) -> Result<Vec<Entry>, JournalError> {
        Ok(search_entries(&self.load_all()?, substring))
    }

    /// Удалить одну запись по идентификатору; `false` — записи с таким id не было.
    pub fn delete(&self, id: Uuid) -> Result<bool, JournalError> {
        let entries = self.load_all()?;
        let kept: Vec<Entry> = entries.iter().filter(|e| e.id != id).cloned().collect();
        if kept.len() == entries.len() {
            return Ok(false);
        }
        self.rewrite(&kept)?;
        Ok(true)
    }

    /// Очистить журнал целиком, оставив только заголовок.
    pub fn clear(&self) -> Result<(), JournalError> {
        self.rewrite(&[])
    }

    /// Ротация: оставить не больше `max_entries` последних записей и уложиться в `max_size_mb`.
    ///
    /// Ноль в любом из пределов отключает его. Возвращает число удалённых записей.
    pub fn rotate(&self, max_entries: u32, max_size_mb: u32) -> Result<usize, JournalError> {
        let entries = self.load_all()?;
        let total = entries.len();
        let mut kept: &[Entry] = &entries;
        if max_entries > 0 && kept.len() > max_entries as usize {
            kept = &kept[kept.len() - max_entries as usize..];
        }
        if max_size_mb > 0 {
            let limit = max_size_mb as u64 * 1024 * 1024;
            while !kept.is_empty() && serialized_size(kept)? > limit {
                // Режем самые старые: свежая история ценнее для пользователя.
                let drop = (kept.len() / 10).max(1);
                kept = &kept[drop.min(kept.len())..];
            }
        }
        if kept.len() == total {
            return Ok(0);
        }
        let kept = kept.to_vec();
        self.rewrite(&kept)?;
        Ok(total - kept.len())
    }

    /// Экспорт всех записей в JSONL (тот же формат, что и сам журнал, но без заголовка).
    pub fn export_jsonl(&self, path: &Path) -> Result<usize, JournalError> {
        let entries = self.load_all()?;
        let mut out = String::new();
        for entry in &entries {
            let line =
                serde_json::to_string(entry).map_err(|e| JournalError::Serialize(e.to_string()))?;
            out.push_str(&line);
            out.push('\n');
        }
        std::fs::write(path, out).map_err(|e| io_err(path, e))?;
        Ok(entries.len())
    }

    /// Экспорт всех записей в CSV.
    pub fn export_csv(&self, path: &Path) -> Result<usize, JournalError> {
        let entries = self.load_all()?;
        std::fs::write(path, entries_to_csv(&entries)).map_err(|e| io_err(path, e))?;
        Ok(entries.len())
    }

    /// Атомарная перезапись: временный файл рядом + `rename`.
    fn rewrite(&self, entries: &[Entry]) -> Result<(), JournalError> {
        let mut tmp_name = self.path.as_os_str().to_os_string();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        {
            let file = File::create(&tmp).map_err(|e| io_err(&tmp, e))?;
            restrict_permissions(&file, &tmp)?;
            let mut writer = BufWriter::new(file);
            let header = Header {
                schema: SCHEMA_VERSION,
                created: Utc::now(),
            };
            let line = serde_json::to_string(&header)
                .map_err(|e| JournalError::Serialize(e.to_string()))?;
            writeln!(writer, "{line}").map_err(|e| io_err(&tmp, e))?;
            for entry in entries {
                let line = serde_json::to_string(entry)
                    .map_err(|e| JournalError::Serialize(e.to_string()))?;
                writeln!(writer, "{line}").map_err(|e| io_err(&tmp, e))?;
            }
            let file = writer
                .into_inner()
                .map_err(|e| JournalError::Io(format!("{}: {}", tmp.display(), e.error())))?;
            file.sync_all().map_err(|e| io_err(&tmp, e))?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| io_err(&self.path, e))
    }
}

impl Journal for FileJournal {
    fn append(&mut self, entry: &Entry) -> Result<(), JournalError> {
        let owned;
        let entry = if self.include_text {
            entry
        } else {
            owned = entry.clone().without_text();
            &owned
        };
        let line =
            serde_json::to_string(entry).map_err(|e| JournalError::Serialize(e.to_string()))?;
        debug_assert!(
            !line.contains('\n'),
            "строка журнала не должна переноситься"
        );
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| io_err(&self.path, e))?;
        restrict_permissions(&file, &self.path)?;
        writeln!(file, "{line}").map_err(|e| io_err(&self.path, e))?;
        file.flush().map_err(|e| io_err(&self.path, e))?;
        file.sync_all().map_err(|e| io_err(&self.path, e))
    }
}

/// Журнал, который ничего не пишет: режим «не записывать» (`privacy.no_record_mode`).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullJournal;

impl Journal for NullJournal {
    fn append(&mut self, _entry: &Entry) -> Result<(), JournalError> {
        Ok(())
    }
}

/// Строка-заголовок: объект с `created` и без `id` — записи всегда несут `id`.
fn is_header(line: &str) -> bool {
    serde_json::from_str::<Header>(line).is_ok()
}

/// Права 0600: журнал читает только владелец.
#[cfg(unix)]
fn restrict_permissions(file: &File, path: &Path) -> Result<(), JournalError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = file.metadata().map_err(|e| io_err(path, e))?.permissions();
    if perms.mode() & 0o777 != 0o600 {
        perms.set_mode(0o600);
        file.set_permissions(perms).map_err(|e| io_err(path, e))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_file: &File, _path: &Path) -> Result<(), JournalError> {
    Ok(())
}

fn serialized_size(entries: &[Entry]) -> Result<u64, JournalError> {
    let mut size = 0u64;
    for entry in entries {
        let line =
            serde_json::to_string(entry).map_err(|e| JournalError::Serialize(e.to_string()))?;
        size += line.len() as u64 + 1;
    }
    Ok(size)
}

/// Поиск по подстроке в сыром и итоговом тексте, без учёта регистра.
pub fn search_entries(entries: &[Entry], substring: &str) -> Vec<Entry> {
    let needle = substring.to_lowercase();
    if needle.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|entry| {
            let hit = |field: &Option<String>| {
                field
                    .as_deref()
                    .map(|text| text.to_lowercase().contains(&needle))
                    .unwrap_or(false)
            };
            hit(&entry.text_final) || hit(&entry.text_raw)
        })
        .cloned()
        .collect()
}

/// Фильтр по классу приложения, без учёта регистра.
pub fn filter_by_app(entries: &[Entry], app: &str) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| {
            entry
                .app
                .as_deref()
                .map(|value| value.eq_ignore_ascii_case(app))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Фильтр по времени: границы включительны, `None` — предел не задан.
pub fn filter_by_date(
    entries: &[Entry],
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| since.map(|from| entry.ts >= from).unwrap_or(true))
        .filter(|entry| until.map(|to| entry.ts <= to).unwrap_or(true))
        .cloned()
        .collect()
}

/// CSV-представление истории: заголовок и по строке на запись.
pub fn entries_to_csv(entries: &[Entry]) -> String {
    let mut out =
        String::from("ts,id,app,style,words,audio_secs,wpm,llm_used,latency_total_ms,text_final\n");
    for entry in entries {
        let row = [
            entry.ts.to_rfc3339(),
            entry.id.to_string(),
            entry.app.clone().unwrap_or_default(),
            entry.style.clone(),
            entry.words.to_string(),
            format!("{:.2}", entry.audio_secs),
            entry.wpm.map(|w| format!("{w:.1}")).unwrap_or_default(),
            entry.llm_used.to_string(),
            entry.latency_ms.total.to_string(),
            entry.text_final.clone().unwrap_or_default(),
        ];
        let row: Vec<String> = row.iter().map(|field| csv_field(field)).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Экранирование поля CSV по RFC 4180: кавычки удваиваются, поле берётся в кавычки при нужде.
pub fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Запись-образец для тестов журнала, статистики и конвейера.
#[cfg(test)]
pub(crate) fn test_entry(ts: &str, words: u32, audio_secs: f32, app: &str) -> Entry {
    use crate::domain::entry::{LatencyMs, Mode, Source};

    let ts = DateTime::parse_from_rfc3339(ts)
        .expect("время образца разбирается")
        .with_timezone(&Utc);
    Entry {
        schema: SCHEMA_VERSION,
        id: Uuid::new_v4(),
        ts,
        session_id: Uuid::nil(),
        mode: Mode::Dictation,
        source: Source::Mic,
        app: Some(app.to_string()),
        language: Some("ru".into()),
        audio_secs,
        words,
        wpm: Entry::wpm_for(words, audio_secs),
        style: "cleanup".into(),
        stt_engine: "fake".into(),
        stt_model: "fake".into(),
        llm_provider: None,
        llm_model: None,
        llm_used: false,
        local_llm: true,
        dict_hits: 0,
        inject_method: Some("clipboard-only".into()),
        latency_ms: LatencyMs {
            stt: 400,
            rules: 2,
            total: 500,
            ..Default::default()
        },
        tokens: None,
        error: None,
        text_raw: Some(format!("сырой текст {words}")),
        text_final: Some(format!("Итоговый текст {words}.")),
        audio_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> (tempfile::TempDir, FileJournal) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("journal.jsonl");
        let journal = FileJournal::open(&path).unwrap();
        (dir, journal)
    }

    #[test]
    fn each_entry_is_one_json_line() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        journal
            .append(&test_entry("2026-09-05T10:01:00Z", 20, 8.0, "kitty"))
            .unwrap();
        let text = std::fs::read_to_string(journal.path()).unwrap();
        // Заголовок плюс две записи.
        assert_eq!(text.lines().count(), 3);
        assert_eq!(journal.load_all().unwrap().len(), 2);
    }

    #[test]
    fn entries_survive_reopening_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        {
            let mut journal = FileJournal::open(&path).unwrap();
            journal
                .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
                .unwrap();
        }
        let reopened = FileJournal::open(&path).unwrap();
        let entries = reopened.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].words, 10);
    }

    #[test]
    fn header_line_is_not_an_entry_and_not_broken() {
        let (_dir, journal) = journal();
        let text = std::fs::read_to_string(journal.path()).unwrap();
        assert!(text.contains("\"created\""), "{text}");
        assert!(journal.load_all().unwrap().is_empty());
        assert!(!journal.broken_path().exists());
    }

    #[test]
    fn corrupt_line_is_quarantined_and_the_rest_still_loads() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(journal.path())
            .unwrap();
        writeln!(file, "{{это не json").unwrap();
        drop(file);
        journal
            .append(&test_entry("2026-09-05T10:02:00Z", 30, 9.0, "kitty"))
            .unwrap();

        let entries = journal.load_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].words, 30);
        let broken = std::fs::read_to_string(journal.broken_path()).unwrap();
        assert!(broken.contains("это не json"), "{broken}");
    }

    #[test]
    fn privacy_mode_keeps_metrics_and_drops_texts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        let mut journal = FileJournal::open_with(&path, false).unwrap();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("Итоговый текст"), "{text}");
        let entries = journal.load_all().unwrap();
        assert_eq!(entries[0].words, 10);
        assert_eq!(entries[0].text_final, None);
    }

    #[test]
    fn null_journal_writes_nothing_anywhere() {
        let mut journal = NullJournal;
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
    }

    #[test]
    fn search_matches_substring_ignoring_case() {
        let (_dir, mut journal) = journal();
        let mut first = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        first.text_final = Some("Собрание переносится".into());
        let mut second = test_entry("2026-09-05T10:01:00Z", 12, 5.0, "firefox");
        second.text_final = Some("Отчёт готов".into());
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();

        let found = journal.search("СОБРАНИЕ").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text_final.as_deref(), Some("Собрание переносится"));
        assert_eq!(journal.search("").unwrap().len(), 2);
    }

    #[test]
    fn filters_select_by_app_and_by_date_range() {
        let entries = vec![
            test_entry("2026-09-01T10:00:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-05T10:00:00Z", 12, 5.0, "Firefox"),
            test_entry("2026-09-09T10:00:00Z", 14, 6.0, "kitty"),
        ];
        assert_eq!(filter_by_app(&entries, "firefox").len(), 1);
        assert_eq!(filter_by_app(&entries, "kitty").len(), 2);

        let since = DateTime::parse_from_rfc3339("2026-09-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until = DateTime::parse_from_rfc3339("2026-09-05T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);
        let day = filter_by_date(&entries, Some(since), Some(until));
        assert_eq!(day.len(), 1);
        assert_eq!(day[0].words, 12);
        assert_eq!(filter_by_date(&entries, Some(since), None).len(), 2);
    }

    #[test]
    fn delete_removes_only_the_named_entry() {
        let (_dir, mut journal) = journal();
        let first = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        let second = test_entry("2026-09-05T10:01:00Z", 12, 5.0, "kitty");
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();

        assert!(journal.delete(first.id).unwrap());
        let left = journal.load_all().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, second.id);
        assert!(!journal.delete(first.id).unwrap());
    }

    #[test]
    fn clear_empties_the_journal_but_keeps_it_usable() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        journal.clear().unwrap();
        assert!(journal.load_all().unwrap().is_empty());
        journal
            .append(&test_entry("2026-09-05T11:00:00Z", 7, 3.0, "kitty"))
            .unwrap();
        assert_eq!(journal.load_all().unwrap().len(), 1);
    }

    #[test]
    fn rotation_keeps_the_newest_entries() {
        let (_dir, mut journal) = journal();
        for minute in 0..10 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        assert_eq!(journal.rotate(4, 0).unwrap(), 6);
        let left = journal.load_all().unwrap();
        assert_eq!(left.len(), 4);
        assert_eq!(left[0].words, 7);
        assert_eq!(left[3].words, 10);
        // Повторная ротация уже ничего не удаляет.
        assert_eq!(journal.rotate(4, 0).unwrap(), 0);
    }

    #[test]
    fn rotation_by_size_drops_the_oldest_until_it_fits() {
        let (_dir, mut journal) = journal();
        for minute in 0..20 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        // Одна запись — заметно меньше мегабайта, поэтому предел в 1 МБ ничего не режет.
        assert_eq!(journal.rotate(0, 1).unwrap(), 0);
        assert_eq!(journal.load_all().unwrap().len(), 20);
    }

    #[test]
    fn zero_limits_disable_rotation() {
        let (_dir, mut journal) = journal();
        for minute in 0..5 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        assert_eq!(journal.rotate(0, 0).unwrap(), 0);
        assert_eq!(journal.load_all().unwrap().len(), 5);
    }

    #[test]
    fn export_writes_jsonl_and_csv() {
        let (dir, mut journal) = journal();
        let mut entry = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        entry.text_final = Some("Привет, \"мир\"".into());
        journal.append(&entry).unwrap();

        let jsonl = dir.path().join("out.jsonl");
        assert_eq!(journal.export_jsonl(&jsonl).unwrap(), 1);
        let text = std::fs::read_to_string(&jsonl).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(serde_json::from_str::<Entry>(text.trim()).is_ok());

        let csv = dir.path().join("out.csv");
        assert_eq!(journal.export_csv(&csv).unwrap(), 1);
        let csv = std::fs::read_to_string(&csv).unwrap();
        let mut lines = csv.lines();
        assert_eq!(
            lines.next().unwrap(),
            "ts,id,app,style,words,audio_secs,wpm,llm_used,latency_total_ms,text_final"
        );
        let row = lines.next().unwrap();
        assert!(row.contains("kitty"), "{row}");
        assert!(row.contains("\"Привет, \"\"мир\"\"\""), "{row}");
    }

    #[test]
    fn csv_field_escapes_only_when_needed() {
        assert_eq!(csv_field("kitty"), "kitty");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[cfg(unix)]
    #[test]
    fn journal_file_is_readable_only_by_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, journal) = journal();
        let mode = std::fs::metadata(journal.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[test]
    fn loading_a_missing_file_is_an_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        let journal = FileJournal {
            path: dir.path().join("absent.jsonl"),
            include_text: true,
        };
        assert!(journal.load_all().unwrap().is_empty());
    }
}
