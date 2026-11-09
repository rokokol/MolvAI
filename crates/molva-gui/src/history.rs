// SPDX-License-Identifier: MIT
//! Reading and editing of the phrase journal for the History tab
//!
//! The journal itself lives in `molva_core::app::journal`: this module names the data
//! directory, opens the file by the settings and maps a request of the window to the same
//! filters the CLI uses. A second parser or a second matcher here would answer differently
//! from `molva history` on the same file.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use molva_core::app::journal::{self, FileJournal};
use molva_core::domain::Entry;
use molva_core::Config;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("cannot determine the user data directory")]
    NoDataDir,
    #[error("journal: {0}")]
    Journal(String),
}

impl From<molva_core::domain::journal::JournalError> for HistoryError {
    fn from(error: molva_core::domain::journal::JournalError) -> Self {
        HistoryError::Journal(error.to_string())
    }
}

/// Data directory: `$XDG_DATA_HOME/molva` and its equivalents
pub fn data_dir() -> Result<PathBuf, HistoryError> {
    directories::ProjectDirs::from("", "", "molva")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or(HistoryError::NoDataDir)
}

/// Journal path: from the config, or `<data directory>/journal.jsonl` if the config is empty
pub fn journal_path(config: &Config) -> Result<PathBuf, HistoryError> {
    if !config.journal.path.trim().is_empty() {
        return Ok(PathBuf::from(&config.journal.path));
    }
    Ok(data_dir()?.join("journal.jsonl"))
}

/// All journal entries, newest first. A missing file means an empty history.
/// The entries load by the settings through the core: with `journal.encrypt = true` the texts
/// arrive decrypted, and without a key they arrive empty, but never as ciphertext in the
/// history window
pub fn load_for(config: &Config) -> Result<Vec<Entry>, HistoryError> {
    let journal = FileJournal::open_for(config)?;
    let mut entries = journal.load_all()?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    Ok(entries)
}

/// Delete one phrase. `false` means no entry had this identifier
pub fn delete(path: &Path, id: Uuid) -> Result<bool, HistoryError> {
    Ok(FileJournal::open(path)?.delete(id)?)
}

/// Clear the whole journal
pub fn clear(path: &Path) -> Result<(), HistoryError> {
    Ok(FileJournal::open(path)?.clear()?)
}

/// Selection conditions for the History tab. Empty fields restrict nothing
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Substring, case-insensitive; the search covers the raw and the final text
    pub query: String,
    /// Exact application name from the entry
    pub app: String,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// 0 means no limit
    pub limit: usize,
}

/// Select entries with the core filters, in the order the CLI applies them
///
/// The journal is already a vector of owned entries, so the caller hands it over instead of
/// lending a slice that would be cloned again
pub fn filter_entries(entries: Vec<Entry>, filter: &Filter) -> Vec<Entry> {
    let mut out = entries;
    if !filter.query.trim().is_empty() {
        out = journal::search_entries(&out, &filter.query);
    }
    if !filter.app.trim().is_empty() {
        out = journal::filter_by_app(&out, &filter.app);
    }
    if filter.since.is_some() || filter.until.is_some() {
        out = journal::filter_by_date(&out, filter.since, filter.until);
    }
    // The window shows the newest first, and the limit then keeps the newest
    out.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    if filter.limit > 0 {
        out.truncate(filter.limit);
    }
    out
}

/// Applications that occur in the journal, for the drop-down filter
pub fn apps_of(entries: &[Entry]) -> Vec<String> {
    journal::apps(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
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
            snippet: false,
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
            segments: Vec::new(),
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
    fn a_query_matches_both_texts_and_ignores_case() {
        let filter = Filter {
            query: "привет".into(),
            ..Filter::default()
        };
        let found = filter_entries(sample(), &filter);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn all_conditions_compose() {
        let filter = Filter {
            query: "письмо".into(),
            app: "firefox".into(),
            since: Some(Utc.with_ymd_and_hms(2026, 9, 2, 0, 0, 0).unwrap()),
            ..Filter::default()
        };
        assert_eq!(filter_entries(sample(), &filter).len(), 1);
    }

    #[test]
    fn the_limit_keeps_the_newest_entries() {
        let filter = Filter {
            limit: 1,
            ..Filter::default()
        };
        let found = filter_entries(sample(), &filter);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text_final.as_deref(), Some("ПРИВЕТ ещё раз"));
    }

    #[test]
    fn apps_are_unique_and_alphabetical() {
        assert_eq!(apps_of(&sample()), vec!["firefox", "kitty"]);
    }
}
