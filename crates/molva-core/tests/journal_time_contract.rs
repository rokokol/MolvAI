// SPDX-License-Identifier: MIT
//! The instant in a journal line is a wire format: one build writes it, another reads it, and
//! the file outlives both. These fixtures carry the bytes chrono 0.4 wrote — nanosecond
//! precision, trailing zero digits and all — so a change of the date library has to keep them
//! opening, normalising and comparing the way they did before

// The whole crate is a test: a panic here reports a failure, not an unhandled error
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use molva_core::app::stats::{read_reset_marker, write_reset_marker};
use molva_core::domain::entry::Entry;

/// A journal line with the given instant and the field set a real journal carries. The text is
/// neutral on purpose: this file is published, and only the timestamp is under test
fn line(ts: &str) -> String {
    format!(
        r#"{{"schema":1,"id":"6388bc70-8378-444c-a0ea-6a1b07f9d787","ts":"{ts}","session_id":"ff227e28-ba2c-4561-bbd5-68b9641a54c3","mode":"dictation","source":"mic","app":"kitty","language":"ru","audio_secs":0.224,"words":1,"style":"cleanup","stt_engine":"whisper-cpp","stt_model":"small","llm_used":false,"local_llm":true,"dict_hits":0,"inject_method":"wtype-type","latency_ms":{{"stt":2447,"rules":0,"inject":71,"total":2519}},"text_raw":"a phrase","text_final":"a phrase"}}"#
    )
}

fn parse(ts: &str) -> Entry {
    serde_json::from_str(&line(ts))
        .unwrap_or_else(|error| panic!("the line with {ts:?} must open: {error}"))
}

/// The instant as the journal will write it back, read out of the line rather than from a typed
/// call: the promise is about the bytes on disk, not about the type that produced them
fn rendered(entry: &Entry) -> String {
    let json = serde_json::to_string(entry).expect("an entry always serialises");
    let value: serde_json::Value = serde_json::from_str(&json).expect("and stays valid JSON");
    value["ts"]
        .as_str()
        .expect("ts is a string in the schema")
        .to_string()
}

#[test]
fn a_line_from_the_older_build_keeps_its_instant_to_the_nanosecond() {
    let fixture = "2026-09-05T15:47:19.317523519Z";
    let entry = parse(fixture);
    assert_eq!(rendered(&entry), fixture);
    assert_eq!(entry.text_final.as_deref(), Some("a phrase"));
}

#[test]
fn a_trailing_zero_digit_names_the_same_instant_as_its_trimmed_form() {
    let padded = parse("2026-09-05T15:49:39.682721110Z");
    let trimmed = parse("2026-09-05T15:49:39.68272111Z");
    assert_eq!(padded, trimmed);
    assert_eq!(rendered(&padded), rendered(&trimmed));
}

#[test]
fn whole_seconds_carry_no_fraction_and_a_zero_fraction_is_the_same_second() {
    let entry = parse("2026-09-05T15:49:30Z");
    assert_eq!(rendered(&entry), "2026-09-05T15:49:30Z");
    assert_eq!(entry, parse("2026-09-05T15:49:30.000Z"));
}

#[test]
fn an_offset_from_another_writer_normalises_to_utc() {
    let entry = parse("2026-09-05T18:47:19.317523519+03:00");
    assert_eq!(rendered(&entry), "2026-09-05T15:47:19.317523519Z");
}

#[test]
fn the_rendered_instant_is_stable_under_a_second_round_trip() {
    for fixture in [
        "2026-09-05T15:47:19.317523519Z",
        "2026-09-05T15:49:39.682721110Z",
        "2026-09-05T15:49:30Z",
    ] {
        let once = rendered(&parse(fixture));
        let twice = rendered(&parse(&once));
        assert_eq!(once, twice, "{fixture} must settle on one form");
    }
}

#[test]
fn a_reset_marker_written_by_the_older_build_still_counts() {
    let dir = tempfile::tempdir().expect("a temp directory");
    let journal = dir.path().join("journal.jsonl");
    std::fs::write(
        molva_core::app::stats::reset_marker_path(&journal),
        r#"{"reset_at":"2026-09-05T12:00:00.123456789Z"}"#,
    )
    .expect("the fixture marker");

    let reset_at = read_reset_marker(&journal).expect("the marker must be read");
    assert_eq!(
        rendered(&parse(&reset_at.to_string())),
        "2026-09-05T12:00:00.123456789Z"
    );

    write_reset_marker(&journal, reset_at).expect("the marker must be written");
    let written = std::fs::read_to_string(molva_core::app::stats::reset_marker_path(&journal))
        .expect("the file just written");
    assert_eq!(written, r#"{"reset_at":"2026-09-05T12:00:00.123456789Z"}"#);
}
