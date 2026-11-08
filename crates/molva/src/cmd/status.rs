// SPDX-License-Identifier: MIT
//! `molva status` — the daemon state as one line, as JSON, or as a stream of events

use std::path::Path;

use molva_core::infra::ipc::{Client, IpcClientError};
use molva_core::ipc::protocol::{Command, Event, WorkerJob};
use serde_json::Value;

/// A status line for a person
///
/// The state machine and the job of the worker thread are two facts: a file transcription
/// leaves the state `idle` while the recognizer is busy, and a line that says only "ready"
/// would be wrong
pub(crate) fn describe(status: &Value) -> String {
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let pid = status.get("pid").and_then(Value::as_u64).unwrap_or(0);
    let job = status
        .get("job")
        .and_then(Value::as_str)
        .map(|job| format!(", {}", human_job(job)))
        .unwrap_or_default();
    format!("state: {}{job} (pid {pid})", human_state(state))
}

/// The protocol states in plain words: the user sees them, not the developer
pub(crate) fn human_state(state: &str) -> &str {
    match state {
        "idle" => "ready",
        "recording" => "recording",
        "transcribing" => "transcribing",
        "post_processing" => "post-processing",
        "injecting" => "injecting",
        other => other,
    }
}

/// The job of the worker thread in plain words
pub(crate) fn human_job(job: &str) -> &str {
    match job {
        "file" => "transcribing a file",
        other => other,
    }
}

/// A short line about an event for `--watch`
pub(crate) fn describe_event(event: &Event) -> Option<String> {
    match event {
        Event::State { state, job, .. } => {
            let name = serde_json::to_value(state)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let job = match job {
                Some(WorkerJob::File) => ", transcribing a file",
                None => "",
            };
            Some(format!("{}{job}", human_state(&name)))
        }
        Event::Entry { entry } => Some(format!(
            "phrase: {} ({} words, {} ms)",
            entry.text_final.as_deref().unwrap_or("<no text>"),
            entry.words,
            entry.latency_ms.total
        )),
        Event::Error { message, .. } => Some(format!("error: {message}")),
        // The signal level comes dozens of times per second: it does not fit into a text output
        Event::Level { .. } | Event::Hypothesis { .. } => None,
        Event::ConfigReloaded => Some("settings reloaded".into()),
        Event::DevicesChanged => Some("the device list changed".into()),
    }
}

pub(crate) fn run(socket: &Path, json: bool, watch: bool) -> Result<(), IpcClientError> {
    let mut client = Client::connect(socket)?;
    let status = client.call_ok(Command::Status)?;
    if json {
        println!("{status}");
    } else {
        println!("{}", describe(&status));
    }
    if !watch {
        return Ok(());
    }
    let events = Client::connect(socket)?.subscribe(false)?;
    for event in events {
        if json {
            if let Ok(line) = serde_json::to_string(&event) {
                println!("{line}");
            }
        } else if let Some(line) = describe_event(&event) {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::ipc::protocol::DaemonState;

    #[test]
    fn status_is_rendered_with_state_and_pid() {
        let status = serde_json::json!({ "state": "recording", "pid": 17 });
        assert_eq!(describe(&status), "state: recording (pid 17)");
    }

    #[test]
    fn an_unknown_state_is_shown_as_is_instead_of_being_hidden() {
        let status = serde_json::json!({ "state": "warming_up", "pid": 1 });
        assert!(describe(&status).contains("warming_up"));
    }

    #[test]
    fn a_file_job_is_named_beside_the_idle_state() {
        let status = serde_json::json!({ "state": "idle", "job": "file", "pid": 17 });
        assert_eq!(
            describe(&status),
            "state: ready, transcribing a file (pid 17)"
        );
        let none = serde_json::json!({ "state": "idle", "job": null, "pid": 17 });
        assert_eq!(describe(&none), "state: ready (pid 17)");
        assert_eq!(human_job("file"), "transcribing a file");
        assert_eq!(human_job("rendering"), "rendering");
    }

    #[test]
    fn level_events_are_not_printed_but_replies_are() {
        assert_eq!(describe_event(&Event::Level { rms: 0.4 }), None);
        assert_eq!(
            describe_event(&Event::State {
                state: DaemonState::Injecting,
                mode: None,
                job: None,
            }),
            Some("injecting".to_string())
        );
        assert_eq!(
            describe_event(&Event::State {
                state: DaemonState::Idle,
                mode: None,
                job: Some(WorkerJob::File),
            }),
            Some("ready, transcribing a file".to_string())
        );
        assert_eq!(
            describe_event(&Event::Error {
                code: molva_core::ipc::protocol::ErrorCode::InjectFailed,
                message: "no window".into(),
                hint: None
            }),
            Some("error: no window".to_string())
        );
    }
}
