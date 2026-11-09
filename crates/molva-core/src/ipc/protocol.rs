// SPDX-License-Identifier: MIT
//! IPC protocol: JSON, one line is one message
//!
//! Request: `{"v":1,"id":7,"cmd":"record.start","args":{"mode":"dictation"}}`
//! Response: `{"id":7,"ok":true,"result":{...}}` or `{"id":7,"ok":false,"error":{...}}`
//! Events on subscribed connections: `{"event":"level","rms":0.08}`

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::file_result::FileResult;
use crate::domain::entry::{Entry, Mode, Source};

pub const PROTOCOL_VERSION: u32 = 1;

/// Daemon state that the clients can see
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Idle,
    Recording,
    Transcribing,
    PostProcessing,
    Injecting,
}

/// What the worker thread does beside the recording state machine
///
/// A file transcription holds the recognizer while the state machine stays `Idle`: the person
/// can still record, the phrase waits in the queue, and the client must see why nothing is
/// processed at once. So the job is a field of the status, not a state of the machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerJob {
    /// The worker transcribes a file for `file.transcribe`
    File,
}

/// Client commands. Names with a dot are namespaces, as in the CLI
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args")]
pub enum Command {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "record.start")]
    RecordStart {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    #[serde(rename = "record.stop")]
    RecordStop,
    #[serde(rename = "record.toggle")]
    RecordToggle {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    #[serde(rename = "record.cancel")]
    RecordCancel,
    #[serde(rename = "style.set")]
    StyleSet { style: String },
    #[serde(rename = "style.next")]
    StyleNext,
    #[serde(rename = "config.reload")]
    ConfigReload,
    #[serde(rename = "devices.list")]
    DevicesList,
    #[serde(rename = "dictionary.reload")]
    DictionaryReload,
    #[serde(rename = "inject.text")]
    InjectText {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<crate::domain::inject::OutputMode>,
    },
    /// Transcribe one audio or video file with the recognizer and the pipeline that the daemon
    /// already holds: no client loads a model of its own
    #[serde(rename = "file.transcribe")]
    FileTranscribe(FileTranscriptionRequest),
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(default)]
        levels: bool,
    },
    #[serde(rename = "shutdown")]
    Shutdown,
}

/// What a file transcription asks for
///
/// The recognizer, its language, the dictionary, the rules and the model are the settings of
/// the daemon: a request carries only what the caller picks for this one file. The source goes
/// with the request, so the journal line of an MCP client and of the CLI stay apart without a
/// second command for each
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTranscriptionRequest {
    /// Path on the machine where the daemon runs
    pub path: PathBuf,
    /// Post-processing style; the default style of the settings when absent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Segments with timecodes in the answer
    #[serde(default = "default_true")]
    pub timecodes: bool,
    /// Do not call the post-processing model for this file
    #[serde(default)]
    pub no_llm: bool,
    /// What the journal records: `mcp` for an MCP client, `file` for the CLI
    #[serde(default = "default_source")]
    pub source: Source,
    /// Cut the decoded file at pauses before recognition
    #[serde(default)]
    pub chunked: bool,
}

fn default_true() -> bool {
    true
}

fn default_source() -> Source {
    Source::File
}

/// The answer of `file.transcribe`: the same shape that `molva transcribe --json` prints
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileReport {
    #[serde(default)]
    pub results: Vec<FileResult>,
    #[serde(default)]
    pub errors: Vec<FileFailure>,
}

impl FileReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// One input that could not be transcribed and why
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFailure {
    pub file: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

impl Request {
    pub fn new(id: u64, cmd: Command) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            cmd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Busy,
    NotRecording,
    NoDevice,
    SttFailed,
    LlmFailed,
    InjectFailed,
    BadRequest,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
    /// What to do next — it goes to the notification for the user
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, code: ErrorCode, message: impl Into<String>, hint: Option<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(IpcError {
                code,
                message: message.into(),
                hint,
            }),
        }
    }
}

/// Events for subscribers
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    State {
        state: DaemonState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<Mode>,
        /// A file transcription holds the worker while the state machine stays as it is
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job: Option<WorkerJob>,
    },
    Level {
        rms: f32,
    },
    /// Draft of the text during the recording (streaming preview)
    Hypothesis {
        text: String,
    },
    /// The entry is boxed: it is an order of magnitude larger than the other events
    Entry {
        entry: Box<Entry>,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    ConfigReloaded,
    /// The default post-processing style changed, and its identifier
    StyleChanged {
        style: String,
    },
    DevicesChanged,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(cmd: &Command) {
        let request = Request::new(1, cmd.clone());
        let json = serde_json::to_string(&request).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(&back.cmd, cmd, "mismatch after round-trip: {json}");
    }

    #[test]
    fn every_command_round_trips() {
        round_trip(&Command::Ping);
        round_trip(&Command::Status);
        round_trip(&Command::RecordStart {
            mode: Mode::Dictation,
            style: Some("cleanup".into()),
        });
        round_trip(&Command::RecordStop);
        round_trip(&Command::RecordToggle {
            mode: Mode::Command,
            style: None,
        });
        round_trip(&Command::RecordCancel);
        round_trip(&Command::StyleSet {
            style: "formal".into(),
        });
        round_trip(&Command::StyleNext);
        round_trip(&Command::ConfigReload);
        round_trip(&Command::DevicesList);
        round_trip(&Command::DictionaryReload);
        round_trip(&Command::InjectText {
            text: "текст".into(),
            mode: Some(crate::domain::inject::OutputMode::Paste),
        });
        round_trip(&Command::Subscribe { levels: true });
        round_trip(&Command::Shutdown);
    }

    fn request_fixture() -> FileTranscriptionRequest {
        FileTranscriptionRequest {
            path: PathBuf::from("/tmp/lecture.mp4"),
            style: Some("cleanup".into()),
            timecodes: true,
            no_llm: false,
            source: Source::Mcp,
            chunked: false,
        }
    }

    #[test]
    fn a_file_transcription_round_trips_with_its_arguments() {
        round_trip(&Command::FileTranscribe(request_fixture()));
        let json =
            serde_json::to_value(Request::new(3, Command::FileTranscribe(request_fixture())))
                .unwrap();
        assert_eq!(json["cmd"], "file.transcribe");
        assert_eq!(json["args"]["path"], "/tmp/lecture.mp4");
        assert_eq!(json["args"]["style"], "cleanup");
        assert_eq!(json["args"]["timecodes"], true);
        assert_eq!(json["args"]["no_llm"], false);
        assert_eq!(json["args"]["source"], "mcp");
        assert_eq!(json["args"]["chunked"], false);
    }

    #[test]
    fn a_file_request_carries_defaults_for_the_optional_arguments() {
        // A client that sends only the path gets timecodes and the journal source of the CLI
        let request: Request = serde_json::from_str(
            r#"{"v":1,"id":1,"cmd":"file.transcribe","args":{"path":"a.wav"}}"#,
        )
        .unwrap();
        let Command::FileTranscribe(args) = request.cmd else {
            panic!("the file command was expected");
        };
        assert_eq!(args.path, PathBuf::from("a.wav"));
        assert_eq!(args.style, None);
        assert!(args.timecodes);
        assert!(!args.no_llm);
        assert_eq!(args.source, Source::File);
        assert!(!args.chunked);
    }

    #[test]
    fn the_file_report_holds_the_results_and_the_failures() {
        let report = FileReport {
            results: vec![FileResult {
                file: "/tmp/a.wav".into(),
                text: "привет".into(),
                segments: Vec::new(),
                audio_secs: 1.5,
                latency_ms: 12,
                language: Some("ru".into()),
            }],
            errors: vec![FileFailure {
                file: "/tmp/b.wav".into(),
                reason: "unknown format".into(),
            }],
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["results"][0]["text"], "привет");
        assert_eq!(json["errors"][0]["file"], "/tmp/b.wav");
        assert_eq!(json["errors"][0]["reason"], "unknown format");
        let back: FileReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
        assert!(!report.ok());
        assert!(FileReport::default().ok());
    }

    #[test]
    fn request_wire_format_matches_documentation() {
        let request = Request::new(
            7,
            Command::RecordStart {
                mode: Mode::Dictation,
                style: None,
            },
        );
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["id"], 7);
        assert_eq!(json["cmd"], "record.start");
        assert_eq!(json["args"]["mode"], "dictation");
        let ping = serde_json::to_string(&Request::new(1, Command::Ping)).unwrap();
        assert_eq!(ping, r#"{"v":1,"id":1,"cmd":"ping"}"#);
    }

    #[test]
    fn unknown_command_is_rejected() {
        let err = serde_json::from_str::<Request>(r#"{"v":1,"id":1,"cmd":"fly"}"#);
        assert!(err.is_err());
    }

    #[test]
    fn error_response_carries_code_and_hint() {
        let response = Response::err(
            3,
            ErrorCode::Busy,
            "recording is already running",
            Some("press the key again to stop".into()),
        );
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "busy");
        assert!(json.get("result").is_none());
        let back: Response = serde_json::from_value(json).unwrap();
        assert_eq!(back, response);
    }

    #[test]
    fn events_are_tagged_by_name() {
        let level = serde_json::to_value(Event::Level { rms: 0.5 }).unwrap();
        assert_eq!(level["event"], "level");
        let state = serde_json::to_value(Event::State {
            state: DaemonState::PostProcessing,
            mode: Some(Mode::Dictation),
            job: None,
        })
        .unwrap();
        assert_eq!(state["state"], "post_processing");
        assert!(state.get("job").is_none(), "an absent job is not sent");
        let busy = serde_json::to_value(Event::State {
            state: DaemonState::Idle,
            mode: None,
            job: Some(WorkerJob::File),
        })
        .unwrap();
        assert_eq!(busy["job"], "file");
        // A client of an older build sends no job and must still be understood
        let old: Event = serde_json::from_str(r#"{"event":"state","state":"idle"}"#).unwrap();
        assert_eq!(
            old,
            Event::State {
                state: DaemonState::Idle,
                mode: None,
                job: None
            }
        );
        let reloaded = serde_json::to_string(&Event::ConfigReloaded).unwrap();
        assert_eq!(reloaded, r#"{"event":"config_reloaded"}"#);
    }
}
