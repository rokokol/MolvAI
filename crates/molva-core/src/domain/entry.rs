// SPDX-License-Identifier: MIT
//! Journal entry of a phrase — the only store of data about dictations
//!
//! The schema is in `docs/journal-schema.md`; the `schema` field sets the version of the line

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current schema version of a journal line
pub const SCHEMA_VERSION: u32 = 1;

/// A phrase shorter than this gets no WPM: division by a fraction of a second gives nonsense
/// numbers
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
    /// A file transcription requested by an MCP client: still a file for search, separate in
    /// the statistics
    Mcp,
}

/// Stage latencies in milliseconds. A missing stage is `None`, not 0
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LatencyMs {
    pub stt: u32,
    pub rules: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<u32>,
    pub total: u32,
    /// Time to the first hypothesis in the streaming preview
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_hypothesis: Option<u32>,
    /// From the key release to the actual stop of the recording
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_after_release: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub prompt: u32,
    pub completion: u32,
}

/// Serde helper: a counter that is missing from the line when it holds nothing
///
/// Serde passes the field by reference, so the signature follows it rather than the usual
/// advice to take a number by value
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub schema: u32,
    pub id: Uuid,
    /// UTC, RFC 3339
    pub ts: Timestamp,
    pub session_id: Uuid,
    pub mode: Mode,
    pub source: Source,
    /// The phrase came from a snippet and the journal holds the prepared replacement
    #[serde(default, skip_serializing_if = "is_false")]
    pub snippet: bool,
    /// Class or name of the active application, if it could be found
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_provider: Option<String>,
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
    /// What the provider charged for the model calls of this phrase, in US dollars
    ///
    /// A retry is a call the provider bills for, so an answer that was thrown away keeps its
    /// charge here. Absent when no call reported one, which is different from zero: a local
    /// model and an unreachable provider both leave it empty for their own reasons
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_cost_usd: Option<f64>,
    /// Requests that left for the provider, including the ones whose answer was thrown away
    #[serde(default, skip_serializing_if = "is_zero")]
    pub llm_calls: u32,
    /// Calls that came back without a charge, so `llm_cost_usd` is not the whole bill
    #[serde(default, skip_serializing_if = "is_zero")]
    pub llm_unpriced_calls: u32,
    /// What the remote recognizer charged for this phrase, in US dollars
    ///
    /// Only a provider that reports a price fills it; a local engine leaves it empty, because
    /// its zero is a property of the settings and not a price of the answer
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_cost_usd: Option<f64>,
    /// A remote recognition that came back without a charge, so `stt_cost_usd` is not the whole
    /// bill of recognition
    #[serde(default, skip_serializing_if = "is_false")]
    pub stt_unpriced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Absent in privacy mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_final: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
    /// Segments with timecodes of a file transcription; empty for a phrase from the microphone
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<crate::domain::stt::Segment>,
}

impl Entry {
    /// Words per minute of audio; `None` for a phrase shorter than a second
    pub fn wpm_for(words: u32, audio_secs: f32) -> Option<f32> {
        if audio_secs < MIN_AUDIO_SECS_FOR_WPM {
            return None;
        }
        Some(words as f32 / audio_secs * 60.0)
    }

    /// Copy with no texts and no audio path — for the journal in privacy mode
    #[must_use]
    pub fn without_text(mut self) -> Self {
        self.text_raw = None;
        self.text_final = None;
        self.audio_path = None;
        self.segments.clear();
        self
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Entry {
        Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::nil(),
            ts: "2026-09-05T12:00:00Z".parse().expect("a fixed instant"),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::Mic,
            snippet: false,
            app: Some("kitty".into()),
            language: Some("ru".into()),
            audio_secs: 4.0,
            words: 10,
            wpm: Entry::wpm_for(10, 4.0),
            style: "cleanup".into(),
            stt_engine: "fake".into(),
            stt_provider: None,
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
            llm_cost_usd: None,
            llm_calls: 0,
            llm_unpriced_calls: 0,
            stt_cost_usd: None,
            stt_unpriced: false,
            error: None,
            text_raw: Some("привет мир".into()),
            text_final: Some("Привет, мир.".into()),
            audio_path: None,
            segments: Vec::new(),
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
        assert!(!line.contains("\"snippet\""));
        assert!(line.contains("\"stt\":500"));
    }

    #[test]
    fn a_snippet_entry_carries_its_mark_through_json() {
        let mut entry = sample();
        entry.snippet = true;
        let line = serde_json::to_string(&entry).unwrap();
        assert!(line.contains("\"snippet\":true"));
        let back: Entry = serde_json::from_str(&line).unwrap();
        assert!(back.snippet);
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

    #[test]
    fn the_charge_of_the_recognizer_reaches_the_line() {
        let mut entry = sample();
        entry.stt_cost_usd = Some(0.000_123);
        let line = serde_json::to_string(&entry).unwrap();
        assert!(line.contains("\"stt_cost_usd\":0.000123"), "{line}");
        let back: Entry = serde_json::from_str(&line).unwrap();
        assert_eq!(back.stt_cost_usd, Some(0.000_123));
    }

    #[test]
    fn a_line_from_before_the_cost_of_recognition_reads_as_a_known_zero() {
        // The journal of an older version has no such fields, and a missing field is not a damaged
        // line: serde fills in nothing, which is what the aggregation expects
        let mut line: serde_json::Value = serde_json::to_value(sample()).unwrap();
        let object = line.as_object_mut().unwrap();
        object.remove("stt_cost_usd");
        object.remove("stt_unpriced");
        let back: Entry = serde_json::from_value(line).unwrap();
        assert_eq!(back.stt_cost_usd, None);
        assert!(!back.stt_unpriced);
    }

    #[test]
    fn a_remote_recognition_that_was_not_priced_is_serialized() {
        let mut entry = sample();
        entry.stt_unpriced = true;
        let line = serde_json::to_string(&entry).unwrap();
        assert!(line.contains("\"stt_unpriced\":true"), "{line}");
        let back: Entry = serde_json::from_str(&line).unwrap();
        assert!(back.stt_unpriced);
    }
}
