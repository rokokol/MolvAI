// SPDX-License-Identifier: MIT
//! Запись журнала реплики — единственное хранилище данных о диктовках.
//!
//! Схема описана в `docs/journal-schema.md`; поле `schema` версионирует строку.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Текущая версия схемы строки журнала.
pub const SCHEMA_VERSION: u32 = 1;

/// Реплика короче этого не получает WPM: деление на долю секунды даёт бессмысленные числа.
pub const MIN_AUDIO_SECS_FOR_WPM: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Dictation,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Mic,
    File,
}

/// Задержки стадий в миллисекундах. Отсутствующая стадия — `None`, а не 0.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LatencyMs {
    pub stt: u32,
    pub rules: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<u32>,
    pub total: u32,
    /// Время до первой гипотезы при потоковом предпросмотре.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_hypothesis: Option<u32>,
    /// От отпускания клавиши до фактической остановки записи.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_after_release: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub prompt: u32,
    pub completion: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub schema: u32,
    pub id: Uuid,
    /// UTC, RFC 3339.
    pub ts: DateTime<Utc>,
    pub session_id: Uuid,
    pub mode: Mode,
    pub source: Source,
    /// Класс/имя активного приложения, если удалось определить.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub audio_secs: f32,
    pub words: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wpm: Option<f32>,
    pub style: String,
    pub stt_engine: String,
    pub stt_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    pub llm_used: bool,
    pub local_llm: bool,
    pub dict_hits: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject_method: Option<String>,
    pub latency_ms: LatencyMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Отсутствуют в режиме приватности.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_final: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
}

impl Entry {
    /// Слов в минуту по длительности аудио; `None` для реплик короче секунды.
    pub fn wpm_for(words: u32, audio_secs: f32) -> Option<f32> {
        if audio_secs < MIN_AUDIO_SECS_FOR_WPM {
            return None;
        }
        Some(words as f32 / audio_secs * 60.0)
    }

    /// Копия без текстов и пути к аудио — для журнала в режиме приватности.
    #[must_use]
    pub fn without_text(mut self) -> Self {
        self.text_raw = None;
        self.text_final = None;
        self.audio_path = None;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Entry {
        Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::nil(),
            ts: DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::Mic,
            app: Some("kitty".into()),
            language: Some("ru".into()),
            audio_secs: 4.0,
            words: 10,
            wpm: Entry::wpm_for(10, 4.0),
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
                stt: 500,
                rules: 2,
                total: 600,
                ..Default::default()
            },
            tokens: None,
            error: None,
            text_raw: Some("привет мир".into()),
            text_final: Some("Привет, мир.".into()),
            audio_path: None,
        }
    }

    #[test]
    fn wpm_is_words_per_minute_of_audio() {
        assert_eq!(Entry::wpm_for(10, 4.0), Some(150.0));
        assert_eq!(Entry::wpm_for(0, 4.0), Some(0.0));
    }

    #[test]
    fn wpm_is_absent_for_audio_shorter_than_a_second() {
        assert_eq!(Entry::wpm_for(3, 0.5), None);
        assert_eq!(Entry::wpm_for(3, 0.0), None);
    }

    #[test]
    fn entry_round_trips_through_json_line() {
        let entry = sample();
        let line = serde_json::to_string(&entry).unwrap();
        assert!(!line.contains('\n'));
        let back: Entry = serde_json::from_str(&line).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn absent_optional_fields_are_not_serialized() {
        let line = serde_json::to_string(&sample()).unwrap();
        assert!(!line.contains("\"llm_provider\""));
        assert!(!line.contains("\"tokens\""));
        assert!(line.contains("\"stt\":500"));
    }

    #[test]
    fn privacy_copy_drops_texts_but_keeps_metrics() {
        let entry = sample().without_text();
        assert_eq!(entry.text_raw, None);
        assert_eq!(entry.text_final, None);
        assert_eq!(entry.words, 10);
        let line = serde_json::to_string(&entry).unwrap();
        assert!(!line.contains("Привет"));
    }
}
