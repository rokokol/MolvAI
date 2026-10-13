// SPDX-License-Identifier: MIT
//! Minimal IPC client of the daemon: newline-delimited JSON over a local socket
//!
//! Replaced by `molva_core::infra::ipc::Client` when track F moves the transport to the core.
//! Here we deliberately keep only what the GUI needs: a one-shot request and a subscription
//! stream

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use interprocess::local_socket::{prelude::*, Stream};
use molva_core::ipc::{Command, ErrorCode, Event, IpcError, Request, Response};
use serde_json::Value;
use thiserror::Error;

use crate::i18n;

/// Counter of request identifiers for the whole process: responses match requests by it
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum IpcClientError {
    #[error("daemon is not running ({path}): {reason}")]
    NotRunning { path: String, reason: String },
    #[error("daemon closed the connection")]
    Disconnected,
    #[error("communication error with the daemon: {0}")]
    Io(String),
    #[error("daemon sent something that the protocol does not describe: {0}")]
    Protocol(String),
    #[error("{0}")]
    Daemon(#[from] DaemonError),
    #[error("cannot determine the daemon socket path: {0}")]
    NoSocketPath(String),
}

impl IpcClientError {
    /// What to offer the user as the next step; it goes to the notice and to the UI
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::NotRunning { .. } | Self::Disconnected => Some(i18n::t("daemon.notRunningHint")),
            Self::Daemon(err) => err.hint.clone(),
            _ => None,
        }
    }

    /// The message for the window and the notice, in the interface language
    ///
    /// `Display` stays English for the log. The failures a user acts on are translated; the
    /// rest are protocol and transport details, kept as they are. A daemon error gets a
    /// translated title by its code, and the daemon's own English text after it
    pub fn user_message(&self) -> String {
        match self {
            Self::NotRunning { path, reason } => {
                i18n::tv("daemon.notRunning", &[("path", path), ("reason", reason)])
            }
            Self::Disconnected => i18n::t("daemon.closed"),
            Self::Daemon(err) => format!("{}: {}", i18n::daemon_error_title(err.code), err.message),
            _ => self.to_string(),
        }
    }

    /// Tells "no daemon" from "the daemon answered with an error": the UI shows them
    /// in different ways
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::NotRunning { .. } | Self::Disconnected)
    }
}

/// An error that came from the daemon through the protocol
#[derive(Debug, Error)]
#[error("{message}")]
pub struct DaemonError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
}

impl From<IpcError> for DaemonError {
    fn from(err: IpcError) -> Self {
        Self {
            code: err.code,
            message: err.message,
            hint: err.hint,
        }
    }
}

/// Daemon socket path: `$XDG_RUNTIME_DIR/molva.sock`, `$TMPDIR/molva.sock`, `\\.\pipe\molva`
pub fn socket_path() -> Result<PathBuf, IpcClientError> {
    if let Some(path) = std::env::var_os("MOLVA_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    {
        Ok(PathBuf::from(r"\\.\pipe\molva"))
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into());
        Ok(PathBuf::from(dir).join("molva.sock"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            IpcClientError::NoSocketPath("the variable XDG_RUNTIME_DIR is not set".into())
        })?;
        Ok(PathBuf::from(dir).join("molva.sock"))
    }
}

/// A message from the daemon: a response to a request or a subscription event
#[derive(Debug)]
pub enum Message {
    Response(Response),
    Event(Event),
}

/// One connection to the daemon. A one-shot request closes it at once, and a subscription keeps it
#[derive(Debug)]
pub struct Connection {
    reader: BufReader<Stream>,
}

impl Connection {
    /// Connect to the daemon socket at the path from the environment
    pub fn connect() -> Result<Self, IpcClientError> {
        Self::connect_at(&socket_path()?)
    }

    /// Connect at a given path: tests use it to supply their own socket
    /// without a change to the environment variables of the process
    pub fn connect_at(path: &Path) -> Result<Self, IpcClientError> {
        let display = path.display().to_string();
        // The core builds the name: on Windows a path from tempdir or `MOLVA_SOCKET` must turn
        // into the same pipe name that the daemon uses
        let name = molva_core::infra::ipc::transport::socket_name(path)
            .map_err(|e| IpcClientError::NoSocketPath(e.to_string()))?;
        let stream = Stream::connect(name).map_err(|e| IpcClientError::NotRunning {
            path: display,
            reason: e.to_string(),
        })?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    /// Send a command without waiting for the response: this is how a subscription opens
    pub fn send(&mut self, cmd: Command) -> Result<u64, IpcClientError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut line = serde_json::to_string(&Request::new(id, cmd))
            .map_err(|e| IpcClientError::Protocol(e.to_string()))?;
        line.push('\n');
        let stream = self.reader.get_mut();
        stream
            .write_all(line.as_bytes())
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        stream
            .flush()
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        Ok(id)
    }

    /// Read the next message; `None` means the daemon closed the connection
    pub fn recv(&mut self) -> Result<Option<Message>, IpcClientError> {
        loop {
            let mut line = String::new();
            let read = self
                .reader
                .read_line(&mut line)
                .map_err(|e| IpcClientError::Io(e.to_string()))?;
            if read == 0 {
                return Ok(None);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return Ok(Some(parse_message(trimmed)?));
        }
    }

    /// A command and the response to it; events that arrive between them are skipped
    pub fn request(&mut self, cmd: Command) -> Result<Value, IpcClientError> {
        let id = self.send(cmd)?;
        loop {
            match self.recv()? {
                None => return Err(IpcClientError::Disconnected),
                Some(Message::Event(_)) => {}
                Some(Message::Response(resp)) if resp.id != id => {}
                Some(Message::Response(resp)) => return unwrap_response(resp),
            }
        }
    }
}

/// Parse a protocol line: first an event (it has an `event` field), then a response
pub fn parse_message(line: &str) -> Result<Message, IpcClientError> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| IpcClientError::Protocol(e.to_string()))?;
    if value.get("event").is_some() {
        let event: Event = serde_json::from_value(value)
            .map_err(|e| IpcClientError::Protocol(format!("event: {e}")))?;
        return Ok(Message::Event(event));
    }
    let response: Response = serde_json::from_value(value)
        .map_err(|e| IpcClientError::Protocol(format!("response: {e}")))?;
    Ok(Message::Response(response))
}

/// A successful response gives `result` (or `null`), and a failed one gives the daemon error
pub fn unwrap_response(resp: Response) -> Result<Value, IpcClientError> {
    if resp.ok {
        return Ok(resp.result.unwrap_or(Value::Null));
    }
    let error = resp.error.unwrap_or(IpcError {
        code: ErrorCode::Internal,
        message: "daemon returned an error without a description".into(),
        hint: None,
    });
    Err(IpcClientError::Daemon(error.into()))
}

/// One-shot request: the connection opens and closes around it
pub fn request(cmd: Command) -> Result<Value, IpcClientError> {
    Connection::connect()?.request(cmd)
}

/// Whether the daemon answers `ping`
pub fn is_running() -> bool {
    request(Command::Ping).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::ipc::DaemonState;

    #[test]
    fn event_line_is_recognised_before_response() {
        let msg = parse_message(r#"{"event":"level","rms":0.25}"#).unwrap();
        match msg {
            Message::Event(Event::Level { rms }) => assert_eq!(rms, 0.25),
            other => panic!("expected a level event, got {other:?}"),
        }
    }

    #[test]
    fn state_event_carries_daemon_state() {
        let msg = parse_message(r#"{"event":"state","state":"recording"}"#).unwrap();
        match msg {
            Message::Event(Event::State { state, .. }) => {
                assert_eq!(state, DaemonState::Recording);
            }
            other => panic!("expected a state event, got {other:?}"),
        }
    }

    #[test]
    fn response_line_is_parsed_as_response() {
        let msg = parse_message(r#"{"id":3,"ok":true,"result":{"state":"idle"}}"#).unwrap();
        match msg {
            Message::Response(resp) => {
                assert_eq!(resp.id, 3);
                assert!(resp.ok);
            }
            other @ Message::Event(_) => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn garbage_line_is_a_protocol_error_not_a_panic() {
        let err = parse_message("not json").unwrap_err();
        assert!(matches!(err, IpcClientError::Protocol(_)), "{err}");
    }

    #[test]
    fn error_response_becomes_daemon_error_with_hint() {
        let resp: Response = serde_json::from_str(
            r#"{"id":1,"ok":false,"error":{"code":"no_device","message":"no microphone","hint":"select a device"}}"#,
        )
        .unwrap();
        let err = unwrap_response(resp).unwrap_err();
        assert_eq!(err.hint().as_deref(), Some("select a device"));
        assert!(!err.is_unavailable());
    }

    #[test]
    fn error_response_without_details_still_reports_something() {
        let resp = Response {
            id: 1,
            ok: false,
            result: None,
            error: None,
        };
        let err = unwrap_response(resp).unwrap_err();
        assert!(err.to_string().contains("without a description"), "{err}");
    }

    #[test]
    fn missing_daemon_offers_starting_it() {
        let err = IpcClientError::NotRunning {
            path: "/run/user/1000/molva.sock".into(),
            reason: "no such file".into(),
        };
        assert!(err.is_unavailable());
        assert!(err.hint().unwrap().contains("molva"));
    }
}
