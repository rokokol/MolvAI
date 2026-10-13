// SPDX-License-Identifier: MIT
//! IPC client against a real socket
//!
//! There is no daemon on the other side yet, so a fake server stands here. It speaks exactly
//! the protocol that `molva_core::ipc` freezes. The tests check what a unit test cannot
//! check: the connection, the order of lines, a broken link

// The whole crate is a test: a panic here reports a failure, not an unhandled error
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Stream};
use molva_core::domain::entry::Mode;
use molva_core::ipc::{Command, DaemonState, ErrorCode, Event, Request, Response};
use molva_gui::ipc::{Connection, IpcClientError, Message};

/// Fake daemon: accepts one connection and answers the lines by a script
struct FakeDaemon {
    path: PathBuf,
    handle: Option<JoinHandle<Vec<Request>>>,
    // The directory lives as long as the server lives: the socket sits inside it
    _dir: tempfile::TempDir,
}

impl FakeDaemon {
    /// `reply` receives the parsed request and returns the lines to send,
    /// or `None`, and then the daemon drops the connection without an answer
    fn start(reply: impl Fn(&Request) -> Option<Vec<String>> + Send + 'static) -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("molva.sock");
        // The core builds the name, as it does for the client: on Windows a tempdir path
        // is not a pipe name
        let name = molva_core::infra::ipc::transport::socket_name(&path).expect("socket name");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("socket created");
        // No ready signal is needed: the socket already listens before `start` returns
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let Some(Ok(stream)) = listener.incoming().next() else {
                return seen;
            };
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(request) = serde_json::from_str::<Request>(line.trim()) else {
                    break;
                };
                let answers = reply(&request);
                seen.push(request);
                let Some(answers) = answers else {
                    break;
                };
                let stream = reader.get_mut();
                let mut broken = false;
                for answer in answers {
                    if stream.write_all(format!("{answer}\n").as_bytes()).is_err() {
                        broken = true;
                        break;
                    }
                }
                if broken || stream.flush().is_err() {
                    break;
                }
            }
            seen
        });
        Self {
            path,
            handle: Some(handle),
            _dir: dir,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Wait for the end of service and return all received requests
    fn requests(mut self) -> Vec<Request> {
        self.handle
            .take()
            .expect("server started")
            .join()
            .expect("server thread did not panic")
    }
}

fn ok_line(id: u64, result: serde_json::Value) -> String {
    serde_json::to_string(&Response::ok(id, result)).expect("response serializes")
}

#[test]
fn request_gets_the_answer_to_its_own_id() {
    let daemon = FakeDaemon::start(|request| {
        Some(vec![ok_line(
            request.id,
            serde_json::json!({"state": "idle"}),
        )])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    let result = connection.request(Command::Status).expect("response");
    assert_eq!(result["state"], "idle");

    drop(connection);
    let seen = daemon.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].cmd, Command::Status);
    assert_eq!(seen[0].v, molva_core::ipc::PROTOCOL_VERSION);
}

#[test]
fn events_arriving_before_the_answer_do_not_confuse_the_client() {
    // The daemon may send events at any moment, also between a request and its response
    let daemon = FakeDaemon::start(|request| {
        Some(vec![
            serde_json::to_string(&Event::Level { rms: 0.3 }).unwrap(),
            serde_json::to_string(&Event::State {
                state: DaemonState::Recording,
                mode: Some(Mode::Dictation),
            })
            .unwrap(),
            ok_line(request.id, serde_json::json!({"ok": true})),
        ])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    let result = connection
        .request(Command::RecordStart {
            mode: Mode::Dictation,
            style: None,
        })
        .expect("response");
    assert_eq!(result["ok"], true);
}

#[test]
fn answer_to_a_foreign_id_is_skipped() {
    // The client must skip a response with a foreign identifier, not take it as its own
    let daemon = FakeDaemon::start(|request| {
        Some(vec![
            ok_line(request.id + 1000, serde_json::json!({"foreign": true})),
            ok_line(request.id, serde_json::json!({"own": true})),
        ])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    let result = connection.request(Command::Ping).expect("response");
    assert_eq!(result["own"], true);
    assert!(result.get("foreign").is_none());
}

#[test]
fn daemon_error_carries_code_and_next_step() {
    let daemon = FakeDaemon::start(|request| {
        Some(vec![serde_json::to_string(&Response::err(
            request.id,
            ErrorCode::NoDevice,
            "microphone not found",
            Some("select a device in the settings".into()),
        ))
        .unwrap()])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    let err = connection.request(Command::RecordStop).unwrap_err();
    assert!(err.to_string().contains("microphone"), "{err}");
    assert_eq!(
        err.hint().as_deref(),
        Some("select a device in the settings")
    );
    // A daemon error is not "no daemon": the interface shows them in different ways
    assert!(!err.is_unavailable());
}

#[test]
fn subscription_streams_events_until_the_daemon_closes() {
    let daemon = FakeDaemon::start(|request| match request.cmd {
        Command::Subscribe { levels } => {
            assert!(levels, "the GUI subscribes with levels");
            Some(vec![
                ok_line(request.id, serde_json::Value::Null),
                serde_json::to_string(&Event::State {
                    state: DaemonState::Recording,
                    mode: None,
                })
                .unwrap(),
                serde_json::to_string(&Event::Level { rms: 0.125 }).unwrap(),
                serde_json::to_string(&Event::ConfigReloaded).unwrap(),
            ])
        }
        _ => Some(vec![ok_line(request.id, serde_json::Value::Null)]),
    });

    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    connection
        .send(Command::Subscribe { levels: true })
        .expect("subscription sent");

    let mut events = Vec::new();
    while events.len() < 3 {
        match connection.recv().expect("read") {
            Some(Message::Event(event)) => events.push(event),
            Some(Message::Response(_)) => {}
            None => break,
        }
    }
    assert_eq!(
        events[0],
        Event::State {
            state: DaemonState::Recording,
            mode: None
        }
    );
    assert_eq!(events[1], Event::Level { rms: 0.125 });
    assert_eq!(events[2], Event::ConfigReloaded);
}

#[test]
fn closed_connection_is_reported_not_hung() {
    // The daemon accepted a request and died without an answer: the client must return
    // a broken-link error
    let daemon = FakeDaemon::start(|_| None);
    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    // The first request stays without an answer, and then the server closes the connection
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = connection.request(Command::Ping);
        let _ = tx.send(outcome.is_err());
    });
    drop(daemon.requests());
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the client did not hang on a closed socket"),
        "a broken link must become an error"
    );
}

#[test]
fn missing_socket_is_a_recoverable_absence_of_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let Err(err) = Connection::connect_at(&dir.path().join("no-such.sock")) else {
        panic!("a connection to a socket that does not exist must not succeed");
    };
    assert!(err.is_unavailable(), "{err}");
    assert!(err.hint().is_some(), "the user needs a next step");
    assert!(matches!(err, IpcClientError::NotRunning { .. }));
}

#[test]
fn garbage_from_the_daemon_does_not_crash_the_client() {
    let daemon = FakeDaemon::start(|_| Some(vec!["not json at all".to_string()]));
    let mut connection = Connection::connect_at(daemon.path()).expect("connection");
    let err = connection.request(Command::Ping).unwrap_err();
    assert!(matches!(err, IpcClientError::Protocol(_)), "{err}");
}

#[test]
fn stream_type_is_the_one_the_client_uses() {
    // The test keeps a link to the implementation: if the transport changes, this stops compiling
    fn assert_stream_is_used(
        _: fn(interprocess::local_socket::Name<'_>) -> std::io::Result<Stream>,
    ) {
    }
    assert_stream_is_used(Stream::connect);
}
