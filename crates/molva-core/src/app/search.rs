// SPDX-License-Identifier: MIT
//! Search over the transcripts of files: one implementation for the CLI and the GUI
//!
//! A file transcription differs from a dictated phrase in two ways: it has a source path
//! and, when the engine reports them, segments with timecodes. The journal already holds
//! both, so the search reads the same file and the same filter as the history. No second
//! index exists to drift from the journal.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::entry::{Entry, Source};
use crate::domain::stt::Segment;

/// One match: a whole file or one of its segments
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub entry_id: Uuid,
    /// Source file of the transcription; empty when the journal line did not keep it
    pub file: String,
    pub recorded_at: DateTime<Utc>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u32>,
    /// Ready `MM:SS.mmm` label, so both frontends show the same timecode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timecode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// `12345` ms → `00:12.345`
pub fn format_timecode(ms: u32) -> String {
    let total_secs = ms / 1000;
    format!(
        "{:02}:{:02}.{:03}",
        total_secs / 60,
        total_secs % 60,
        ms % 1000
    )
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(needle)
}

fn hit(entry: &Entry, segment: Option<&Segment>, text: String) -> Hit {
    let (start_ms, end_ms, timecode) = match segment {
        Some(segment) => (
            Some(segment.start_ms),
            Some(segment.end_ms),
            Some(format!(
                "{} → {}",
                format_timecode(segment.start_ms),
                format_timecode(segment.end_ms)
            )),
        ),
        None => (None, None, None),
    };
    Hit {
        entry_id: entry.id,
        file: entry.audio_path.clone().unwrap_or_default(),
        recorded_at: entry.ts,
        text,
        start_ms,
        end_ms,
        timecode,
        language: entry.language.clone(),
    }
}

/// Find a query in the file transcriptions, newest first
///
/// An empty query returns the newest transcriptions whole. A match inside a segment is
/// reported at that segment, with its timecode; a match in text that has no segments is
/// reported as the whole transcription.
pub fn search(entries: &[Entry], query: &str, limit: usize) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    let mut hits = Vec::new();
    for entry in entries.iter().filter(|entry| entry.source == Source::File) {
        let text = entry
            .text_final
            .as_deref()
            .or(entry.text_raw.as_deref())
            .unwrap_or_default();
        if needle.is_empty() {
            if !text.trim().is_empty() {
                hits.push(hit(entry, None, text.to_string()));
            }
            continue;
        }
        let mut matched_segment = false;
        for segment in &entry.segments {
            if contains(&segment.text, &needle) {
                matched_segment = true;
                hits.push(hit(entry, Some(segment), segment.text.trim().to_string()));
            }
        }
        if !matched_segment && contains(text, &needle) {
            hits.push(hit(entry, None, text.to_string()));
        }
    }
    hits.sort_by_key(|hit| std::cmp::Reverse((hit.recorded_at, hit.start_ms.unwrap_or(0))));
    if limit > 0 {
        hits.truncate(limit);
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::{LatencyMs, Mode, SCHEMA_VERSION};
    use chrono::TimeZone;

    fn entry(id: u128, secs: u32, text: &str, segments: Vec<Segment>) -> Entry {
        Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::from_u128(id),
            ts: Utc.timestamp_opt(i64::from(secs), 0).unwrap(),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::File,
            app: None,
            language: Some("ru".into()),
            audio_secs: 10.0,
            words: 2,
            wpm: None,
            style: "raw".into(),
            stt_engine: "transcribe-cpp".into(),
            stt_model: "gigaam".into(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method: None,
            latency_ms: LatencyMs::default(),
            tokens: None,
            error: None,
            text_raw: Some(text.into()),
            text_final: Some(text.into()),
            audio_path: Some(format!("lecture-{id}.mp4")),
            segments,
        }
    }

    fn segment(start: u32, end: u32, text: &str) -> Segment {
        Segment {
            start_ms: start,
            end_ms: end,
            text: text.into(),
        }
    }

    #[test]
    fn a_match_inside_a_segment_carries_its_timecode() {
        let entries = vec![entry(
            1,
            10,
            "первая часть вторая часть",
            vec![
                segment(0, 2000, "первая часть"),
                segment(2000, 4000, "вторая часть"),
            ],
        )];

        let hits = search(&entries, "вторая", 0);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "вторая часть");
        assert_eq!(hits[0].start_ms, Some(2000));
        assert_eq!(hits[0].timecode.as_deref(), Some("00:02.000 → 00:04.000"));
        assert_eq!(hits[0].file, "lecture-1.mp4");
    }

    #[test]
    fn text_without_segments_is_still_searchable() {
        let entries = vec![entry(1, 10, "длинная лекция про физику", vec![])];
        let hits = search(&entries, "физику", 0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start_ms, None);
        assert!(hits[0].timecode.is_none());
    }

    #[test]
    fn dictated_phrases_are_not_in_the_file_search() {
        let mut mic = entry(1, 10, "файл", vec![]);
        mic.source = Source::Mic;
        assert!(search(&[mic], "файл", 0).is_empty());
    }

    #[test]
    fn an_empty_query_returns_newest_first_and_respects_the_limit() {
        let entries = vec![
            entry(1, 10, "старая", vec![]),
            entry(2, 20, "новая", vec![]),
        ];
        let hits = search(&entries, "", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "новая");
    }

    #[test]
    fn case_and_surrounding_spaces_do_not_hide_a_match() {
        let entries = vec![entry(1, 10, "Привет, МИР", vec![])];
        assert_eq!(search(&entries, "  мир ", 0).len(), 1);
    }
}
