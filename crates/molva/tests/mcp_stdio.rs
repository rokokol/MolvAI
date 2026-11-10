// SPDX-License-Identifier: MIT
//! Live check of the MCP server over stdio against a real daemon
//!
//! The process under test is the real `molva` binary: `initialize`, `tools/list` and one
//! `tools/call` per tool, all through the JSON lines that an MCP client sends. The daemon is not
//! started by the test: the server starts it itself, as on a machine where nobody runs one yet,
//! and the test stops it at the end. Nothing outside the temporary directory is touched: the
//! settings, the socket and the journal live there, and the recognizer is the fake engine, so no
//! weights are needed.

// The whole crate is a test: a panic here reports a failure, not an unhandled error
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use molva_core::infra::ipc::{Client, IpcClientError};
use molva_core::ipc::protocol::Command as IpcCommand;
use serde_json::{json, Value};

/// The daemon of one test: stopped when the test ends, however it ends
struct DaemonGuard {
    socket: PathBuf,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Ok(mut client) = Client::connect(&self.socket) {
            let _ = client.call_ok(IpcCommand::Shutdown);
        }
    }
}

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    /// Start the server with the settings and the socket of the test directory
    fn start(config: &Path, socket: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_molva"))
            .arg("--config")
            .arg(config)
            .arg("--socket")
            .arg(socket)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the molva binary starts");
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn send(&mut self, request: &Value) {
        let stdin = self.stdin.as_mut().expect("the session is open");
        writeln!(stdin, "{request}").expect("the request is written");
        stdin.flush().expect("the request leaves");
    }

    /// The next response line, parsed
    fn answer(&mut self) -> Value {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("the answer is read");
        assert!(
            !line.trim().is_empty(),
            "the server closed without an answer"
        );
        serde_json::from_str(&line).expect("the answer is one JSON object")
    }

    /// One `tools/call`, with the payload that a client would parse
    fn call(&mut self, id: u64, name: &str, arguments: &Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }));
        let answer = self.answer();
        assert_eq!(answer["id"], id);
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .expect("a text payload");
        serde_json::from_str(text).expect("the payload is JSON")
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        // The session ends with the input: the server leaves the loop and the process exits
        self.stdin.take();
        let _ = self.child.wait();
    }
}

/// A wav file of a fixed length: the fake engine answers whatever the audio holds
fn write_wav(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("a wav file");
    for index in 0..16_000u32 {
        let value = ((index as f32 * 0.05).sin() * 8_000.0) as i16;
        writer.write_sample(value).expect("a sample");
    }
    writer.finalize().expect("the wav is complete");
}

/// A path as a TOML string: Windows backslashes are escape characters, so they are doubled
fn toml_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

/// The settings of the live test: the fake engine, a journal in the temporary directory
fn write_config(directory: &Path) -> PathBuf {
    write_config_with(directory, true)
}

/// The same settings with the MCP switch set: `false` is what a harness must be refused by
fn write_config_with(directory: &Path, mcp_enabled: bool) -> PathBuf {
    let journal = toml_path(&directory.join("journal.jsonl"));
    let dictionary = toml_path(&directory.join("dictionary.toml"));
    std::fs::write(&dictionary, "").expect("an empty dictionary");
    let config = format!(
        r#"version = 1
ui_language = "en"

[stt]
engine = "fake"
auto_pull = false
model = "small"
language = "ru"

[journal]
path = "{journal}"
enabled = true
include_text = true

[mcp]
enabled = {mcp_enabled}

[privacy]
send_to_llm = false

[llm]
enabled = false

[dictionary]
path = "{dictionary}"

[audio]
device = "default"
warn_zero_level = false

[hotkeys]
backend = "none"
"#
    );
    let path = directory.join("config.toml");
    std::fs::write(&path, config).expect("the settings are written");
    path
}

#[test]
/// A switch that is off stops the server before it reads a line: the harness gets the reason and
/// a status, not a process that waits for an answer to a request nobody will serve
fn a_disabled_mcp_server_refuses_to_start() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let config = write_config_with(directory.path(), false);
    let socket = directory.path().join("molva.sock");

    let output = Command::new(env!("CARGO_BIN_EXE_molva"))
        .arg("--config")
        .arg(&config)
        .arg("--socket")
        .arg(&socket)
        .arg("mcp")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the molva binary runs");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a refused command keeps its own status"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mcp.enabled"),
        "the refusal names the switch: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing speaks JSON-RPC: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    // The daemon is started by a server that runs, so a refused one leaves no socket behind
    assert!(!socket.exists(), "no daemon was started");
}

#[test]
fn every_mcp_tool_answers_through_a_daemon_the_server_starts_itself() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let config = write_config(directory.path());
    let socket = directory.path().join("molva.sock");
    let wav = directory.path().join("lecture.wav");
    write_wav(&wav);
    // The socket belongs to the daemon, but the guard needs its path before the daemon exists
    let _daemon = DaemonGuard {
        socket: socket.clone(),
    };

    let mut mcp = Mcp::start(&config, &socket);

    // 1. initialize
    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": { "name": "live-test", "version": "1" } }
    }));
    let hello = mcp.answer();
    assert_eq!(hello["result"]["serverInfo"]["name"], "molva");
    assert_eq!(hello["result"]["protocolVersion"], "2024-11-05");
    // A notification has no answer: the next line must belong to the next request
    mcp.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    mcp.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }));
    let list = mcp.answer();
    let names: Vec<String> = list["result"]["tools"]
        .as_array()
        .expect("a list of tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();
    for expected in [
        "transcribe",
        "search_transcripts",
        "stats_summary",
        "history_list",
        "models_list",
        "search_docs",
    ] {
        assert!(names.iter().any(|name| name == expected), "{expected}");
    }

    // 3. transcribe: the daemon is started here, because nobody answers on the socket yet
    let report = mcp.call(
        3,
        "transcribe",
        &json!({ "path": wav.display().to_string() }),
    );
    let results = report["results"].as_array().expect("results");
    assert_eq!(results.len(), 1, "{report}");
    assert_eq!(report["errors"], json!([]));
    let text = results[0]["text"].as_str().expect("the text");
    assert!(!text.trim().is_empty(), "the fake engine gave no text");
    assert_eq!(
        results[0]["file"],
        wav.canonicalize().unwrap().display().to_string()
    );
    assert!(
        Client::connect(&socket).is_ok(),
        "the daemon must be up now"
    );

    // 4. search_transcripts sees the line of the journal that the transcription wrote
    let hits = mcp.call(
        4,
        "search_transcripts",
        &json!({ "query": text, "limit": 5 }),
    );
    let hits = hits.as_array().expect("a list of hits");
    assert!(!hits.is_empty(), "the journal entry was not searchable");
    assert_eq!(hits[0]["language"], "ru");

    // 5. stats_summary counts the words of that entry
    let stats = mcp.call(5, "stats_summary", &json!({ "days": 7 }));
    assert!(stats["total_words"].as_u64().unwrap_or(0) > 0, "{stats}");
    assert!(stats["words_today"].as_u64().unwrap_or(0) > 0, "{stats}");

    // 6. history_list: the entry names its file and its source
    let entries = mcp.call(6, "history_list", &json!({ "limit": 5 }));
    let entries = entries.as_array().expect("a list of entries");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["source"], "mcp");
    assert_eq!(
        entries[0]["audio_path"],
        wav.canonicalize().unwrap().display().to_string()
    );

    // 7. models_list: the catalog with the flag of what is on disk
    let models = mcp.call(7, "models_list", &json!({}));
    let models = models.as_array().expect("the catalog");
    assert!(!models.is_empty(), "the catalog is empty");
    assert!(models.iter().all(|model| model["name"].is_string()));

    // 8. search_docs: the documents this binary carries
    let docs = mcp.call(8, "search_docs", &json!({ "query": "daemon", "limit": 3 }));
    assert!(docs["indexed"].as_u64().unwrap_or(0) > 0, "{docs}");
    let found = docs["hits"].as_array().expect("hits");
    assert!(!found.is_empty(), "{docs}");
    assert!(found[0]["url"]
        .as_str()
        .unwrap_or_default()
        .starts_with("https://"));

    // 9. ping and a second session after the first: nothing is left in a bad state
    mcp.send(&json!({ "jsonrpc": "2.0", "id": 9, "method": "ping", "params": {} }));
    assert_eq!(mcp.answer()["result"], json!({}));

    // A tool that does not exist is an error of the call, not of the session
    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": { "name": "no_such_tool", "arguments": {} }
    }));
    let unknown = mcp.answer();
    assert_eq!(unknown["result"]["isError"], true);
}

/// The daemon answers `status` with the socket of the settings, and stops on shutdown
#[test]
fn the_daemon_of_a_live_mcp_session_answers_status_and_stops() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let config = write_config(directory.path());
    let socket = directory.path().join("molva.sock");
    let wav = directory.path().join("short.wav");
    write_wav(&wav);
    let daemon = DaemonGuard {
        socket: socket.clone(),
    };

    let mut mcp = Mcp::start(&config, &socket);
    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "transcribe", "arguments": { "path": wav.display().to_string() } }
    }));
    let mut line = String::new();
    mcp.stdout.read_line(&mut line).expect("an answer");
    // The payload is JSON inside a JSON string: the field name arrives escaped
    assert!(line.contains("results"), "{line}");
    assert!(!line.contains("isError\":true"), "{line}");

    let mut client = Client::connect(&socket).expect("the daemon answers");
    let status = client
        .call_ok(IpcCommand::Status)
        .expect("the status of the daemon");
    assert_eq!(status["state"], "idle");
    // The file job is over by now: its slot is free again
    assert_eq!(status["job"], Value::Null);

    drop(daemon);
    // The daemon answers "shutdown" before it exits: it leaves a moment later
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut stopped = false;
    while std::time::Instant::now() < deadline {
        if matches!(
            Client::connect(&socket),
            Err(IpcClientError::NotRunning { .. })
        ) {
            stopped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(stopped, "the daemon did not stop");
}
