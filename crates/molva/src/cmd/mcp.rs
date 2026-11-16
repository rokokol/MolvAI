// SPDX-License-Identifier: MIT
//! `molva mcp` — a Model Context Protocol server over stdio
//!
//! One JSON-RPC request per line on stdin, one response per line on stdout. There is no port
//! and no listener: the MCP client starts this process itself, so the lifetime and the access
//! are the client's. Settings are read-only here; the tools that change them are deliberately
//! absent.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use molva_core::app::journal::{self, FileJournal};
use molva_core::app::models;
use molva_core::app::search;
use molva_core::app::stats;
use molva_core::config::ConfigError;
use molva_core::domain::entry::Source;
use molva_core::infra::ipc::{Client, IpcClientError};
use molva_core::ipc::protocol::{Command, ErrorCode, FileReport, FileTranscriptionRequest};
use molva_core::Config;
use serde_json::{json, Value};

use crate::docs_index;

use super::daemon::ensure_running;
use super::transcribe::{self, JobSource};
use super::CmdError;

const PROTOCOL_VERSION: &str = "2024-11-05";
const CONFIG_UNAVAILABLE: &str = "cannot load the MCP settings; check the configuration files";
const MCP_DISABLED: &str = "the MCP server is disabled: set mcp.enabled = true in the settings";

/// A search answer is a page of hits rather than the whole index. The schema and the code
/// carry the same numbers, so the schema describes what the code does
const DOCS_LIMIT_DEFAULT: usize = 5;
const DOCS_LIMIT_MAX: usize = 20;

/// How long a `transcribe` call waits for the daemon to finish the phrase it is busy with
///
/// The daemon refuses a file while it records or processes a phrase, because the user is waiting
/// for that phrase and one recognizer serves one job at a time. The refusal is answered at once,
/// and the wait has a bound: an MCP client gets a clear error instead of a call that hangs while
/// the person keeps dictating
const BUSY_WAIT: Duration = Duration::from_secs(30);
const BUSY_STEP: Duration = Duration::from_millis(250);

/// The server's view of one MCP session
struct Context {
    config: Config,
    config_path: PathBuf,
    socket: PathBuf,
}

fn load_mcp_config(path: &Path) -> Result<Config, &'static str> {
    Config::load_strict(path).map_err(|error| match error {
        ConfigError::Read {
            path: failed,
            source,
        } if failed == path
            && source.kind() == std::io::ErrorKind::NotFound
            && std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            MCP_DISABLED
        }
        _ => CONFIG_UNAVAILABLE,
    })
}

impl Context {
    fn snapshot(&self) -> Result<Self, String> {
        let config = load_mcp_config(&self.config_path).map_err(str::to_string)?;
        if !config.mcp.enabled {
            return Err(MCP_DISABLED.to_string());
        }
        Ok(Self {
            config,
            config_path: self.config_path.clone(),
            socket: self.socket.clone(),
        })
    }
}

/// One JSON-RPC request or notification
fn handle(ctx: &Context, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "molva",
                "version": env!("CARGO_PKG_VERSION"),
            }
        })),
        "notifications/initialized" => return None,
        // Both answer with an empty result object: they differ only in when a client sends them
        "ping" | "shutdown" => Ok(json!({})),
        "tools/list" => ctx
            .snapshot()
            .map(|_| json!({ "tools": tools() }))
            .map_err(|message| (-32603, message)),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match ctx
                .snapshot()
                .and_then(|snapshot| call_tool(&snapshot, name, &arguments))
            {
                Ok(value) => Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
                    "isError": false
                })),
                Err(message) => Ok(json!({
                    "content": [{ "type": "text", "text": message }],
                    "isError": true
                })),
            }
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    // A notification has no id and expects no response
    let id = id?;
    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }),
    })
}

/// The tool catalogue. A settings writer is absent on purpose: the GUI and `molva config` own
/// the settings, and an MCP client does not change them
fn tools() -> Value {
    json!([
        {
            "name": "transcribe",
            "description": "Transcribe an audio or video file locally through the running MolvAI daemon, which is started when none answers. Returns the text, the segments with timecodes and the language. The result is written to the journal",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File or directory path" },
                    "model": { "type": "string", "description": "Model name from models.toml; the daemon transcribes with the configured model, so a different name is an error" },
                    "timecodes": { "type": "boolean", "default": true, "description": "Include segments with timecodes" },
                    "style": { "type": "string", "description": "Post-processing style identifier; the default style is used when absent" },
                    "no_llm": { "type": "boolean", "default": false, "description": "Do not call the language model for this run" },
                    "chunked": { "type": "boolean", "default": false, "description": "Cut the file at pauses before recognition, as a live dictation is cut" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "search_transcripts",
            "description": "Search the transcriptions of audio and video files. Returns a hit with the file and the timecode of the segment that matches",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 0, "default": 20 }
                },
                "required": ["query"]
            }
        },
        {
            "name": "stats_summary",
            "description": "Words, speed, latency and the charge the provider reported for the model calls, with the count of calls that reported none; MCP transcriptions are counted separately",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "days": { "type": "integer", "minimum": 0, "default": 7 }
                }
            }
        },
        {
            "name": "history_list",
            "description": "Recent journal entries, optionally filtered by text, application and time",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "app": { "type": "string" },
                    "since": { "type": "string", "description": "RFC 3339 moment" },
                    "until": { "type": "string", "description": "RFC 3339 moment" },
                    "limit": { "type": "integer", "minimum": 0, "default": 20 }
                }
            }
        },
        {
            "name": "models_list",
            "description": "The model catalog and which weights are on disk",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "search_docs",
            "description": "Search the documentation this binary carries: the README, docs/*.md and the site pages. Read-only and offline. Returns the page path, title, language, a snippet around the match and a URL",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "maxLength": 200, "description": "Words to look for; a page holding more of them ranks higher" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": DOCS_LIMIT_MAX, "default": DOCS_LIMIT_DEFAULT },
                    "lang": { "type": "string", "enum": ["en", "ru"], "description": "Keep pages of this language; a translated site page is indexed in both" }
                },
                "required": ["query"]
            }
        }
    ])
}

fn call_tool(ctx: &Context, name: &str, arguments: &Value) -> Result<Value, String> {
    match name {
        "transcribe" => transcribe_tool(ctx, arguments),
        "search_transcripts" => {
            let query = string_arg(arguments, "query")?;
            let limit = usize_arg(arguments, "limit", 20)?;
            let entries = load_entries(ctx)?;
            serde_json::to_value(search::search(&entries, &query, limit)).map_err(|e| e.to_string())
        }
        "stats_summary" => {
            let days = u32_arg(arguments, "days", 7)?;
            let entries = load_entries(ctx)?;
            let summary = stats::summary(&entries, Utc::now(), &ctx.config.stats, days);
            serde_json::to_value(summary).map_err(|e| e.to_string())
        }
        "history_list" => {
            let mut entries = load_entries(ctx)?;
            if let Some(query) = arguments.get("query").and_then(Value::as_str) {
                if !query.trim().is_empty() {
                    entries = journal::search_entries(&entries, query);
                }
            }
            if let Some(app) = arguments.get("app").and_then(Value::as_str) {
                if !app.trim().is_empty() {
                    entries = journal::filter_by_app(&entries, app);
                }
            }
            let since = moment_arg(arguments, "since")?;
            let until = moment_arg(arguments, "until")?;
            if since.is_some() || until.is_some() {
                entries = journal::filter_by_date(&entries, since, until);
            }
            let limit = usize_arg(arguments, "limit", 20)?;
            if limit > 0 {
                entries.truncate(limit);
            }
            serde_json::to_value(entries).map_err(|e| e.to_string())
        }
        "models_list" => {
            let directory = models::models_dir(&ctx.config).map_err(|e| e.to_string())?;
            serde_json::to_value(models::list(&directory)).map_err(|e| e.to_string())
        }
        "search_docs" => {
            let query = string_arg(arguments, "query")?;
            let limit = usize_arg(arguments, "limit", DOCS_LIMIT_DEFAULT)?.clamp(1, DOCS_LIMIT_MAX);
            let lang = lang_arg(arguments)?;
            let outcome = docs_index::search(&query, lang.as_deref(), limit);
            Ok(json!({
                "query": query,
                "lang": lang,
                "indexed": outcome.indexed,
                "hits": outcome.hits.iter().map(|hit| json!({
                    "path": hit.path,
                    "title": hit.title,
                    "lang": hit.lang,
                    "url": hit.url,
                    "score": hit.score,
                    "snippet": hit.snippet,
                })).collect::<Vec<_>>(),
            }))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn transcribe_tool(ctx: &Context, arguments: &Value) -> Result<Value, String> {
    let path = string_arg(arguments, "path")?;
    let timecodes = arguments
        .get("timecodes")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let no_llm = arguments
        .get("no_llm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let chunked = arguments
        .get("chunked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let style = arguments
        .get("style")
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = arguments
        .get("model")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty());
    if let Some(model) = model {
        if model != ctx.config.stt.model {
            return Err(format!(
                "the daemon transcribes with the model of its settings ({}), not with {model}: \
                 change stt.model and restart the daemon, or leave the argument out",
                ctx.config.stt.model
            ));
        }
    }

    // The daemon decodes the file, so the paths must be absolute in its own working directory
    let root = std::fs::canonicalize(&path)
        .map_err(|err| format!("{path}: cannot read the path: {err}"))?;
    let jobs = transcribe::collect_jobs(&[root], false).map_err(|err| err.message)?;

    // A daemon is started once for the whole batch, and only when none answers
    let mut client = ensure_running(&ctx.config_path, &ctx.socket).map_err(|err| err.message)?;
    let mut report = FileReport::default();
    for job in jobs {
        let JobSource::File(source) = job.source else {
            return Err("standard input is not a file, name a path".to_string());
        };
        let request = FileTranscriptionRequest {
            path: source,
            style: style.clone(),
            timecodes,
            no_llm,
            chunked,
            // The journal separates an MCP transcription from a dictation and from the CLI
            source: Source::Mcp,
        };
        let answer = file_request(&mut client, &request, BUSY_WAIT)?;
        report.results.extend(answer.results);
        report.errors.extend(answer.errors);
    }
    serde_json::to_value(report).map_err(|err| err.to_string())
}

/// Ask the daemon for one file and wait out a phrase that is being delivered
///
/// The busy answer is not an error of the request: the daemon is recording or pasting for the
/// person at the keyboard, and the file can wait. Every other failure goes back as it came
fn file_request(
    client: &mut Client,
    request: &FileTranscriptionRequest,
    busy_wait: Duration,
) -> Result<FileReport, String> {
    let deadline = Instant::now() + busy_wait;
    loop {
        match client.call_ok(Command::FileTranscribe(request.clone())) {
            Ok(value) => {
                return serde_json::from_value(value)
                    .map_err(|err| format!("unclear answer from the daemon: {err}"))
            }
            Err(IpcClientError::Daemon(err)) if err.code == ErrorCode::Busy => {
                if Instant::now() >= deadline {
                    return Err(match err.hint {
                        Some(hint) => format!("{}: {hint}", err.message),
                        None => err.message,
                    });
                }
                std::thread::sleep(BUSY_STEP);
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

fn load_entries(ctx: &Context) -> Result<Vec<molva_core::domain::Entry>, String> {
    let journal = FileJournal::open_for(&ctx.config).map_err(|e| e.to_string())?;
    let mut entries = journal.load_all().map_err(|e| e.to_string())?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    Ok(entries)
}

fn string_arg(arguments: &Value, name: &str) -> Result<String, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn usize_arg(arguments: &Value, name: &str, default: usize) -> Result<usize, String> {
    match arguments.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("{name} must be a non-negative integer")),
    }
}

fn u32_arg(arguments: &Value, name: &str, default: u32) -> Result<u32, String> {
    Ok(usize_arg(arguments, name, default as usize)? as u32)
}

fn moment_arg(arguments: &Value, name: &str) -> Result<Option<DateTime<Utc>>, String> {
    match arguments.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .ok_or_else(|| format!("{name} must be an RFC 3339 string"))
            .and_then(|text| {
                DateTime::parse_from_rfc3339(text)
                    .map(|moment| Some(moment.with_timezone(&Utc)))
                    .map_err(|e| format!("{name}: {e}"))
            }),
    }
}

/// The language a search is limited to. The index separates the two languages the documents
/// are written in, and a value outside them would silently return nothing
fn lang_arg(arguments: &Value) -> Result<Option<String>, String> {
    match arguments.get("lang") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_str() {
            Some(lang @ ("en" | "ru")) => Ok(Some(lang.to_string())),
            _ => Err("lang must be \"en\" or \"ru\"".to_string()),
        },
    }
}

/// The stdio loop: one request per line, one response per line, diagnostics on stderr
pub(crate) fn run(config_path: &Path, socket: &Path) -> Result<(), CmdError> {
    let config = load_mcp_config(config_path).map_err(|message| {
        if message == MCP_DISABLED {
            CmdError::args(message)
        } else {
            CmdError::file(message)
        }
    })?;
    if !config.mcp.enabled {
        return Err(CmdError::args(MCP_DISABLED));
    }
    let ctx = Context {
        config,
        config_path: config_path.to_path_buf(),
        socket: socket.to_path_buf(),
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| CmdError::file(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": error.to_string() }
                });
                writeln!(stdout, "{response}").map_err(|e| CmdError::file(e.to_string()))?;
                stdout.flush().map_err(|e| CmdError::file(e.to_string()))?;
                continue;
            }
        };
        if let Some(response) = handle(&ctx, &request) {
            writeln!(stdout, "{response}").map_err(|e| CmdError::file(e.to_string()))?;
            stdout.flush().map_err(|e| CmdError::file(e.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct TestContext {
        ctx: Context,
        _directory: tempfile::TempDir,
    }

    impl std::ops::Deref for TestContext {
        type Target = Context;

        fn deref(&self) -> &Context {
            &self.ctx
        }
    }

    fn context() -> TestContext {
        context_at(Path::new("/tmp/molva-mcp-test.sock"))
    }

    fn context_at(socket: &Path) -> TestContext {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("synthetic.toml");
        let mut config = Config::default();
        config.mcp.enabled = true;
        config.save_plain(&config_path).unwrap();
        assert!(Config::runtime_path(&config_path).unwrap().is_none());
        TestContext {
            ctx: Context {
                config,
                config_path,
                socket: socket.to_path_buf(),
            },
            _directory: directory,
        }
    }

    struct EncryptedJournalFixture {
        directory: tempfile::TempDir,
        ctx: Context,
        entries: Vec<molva_core::domain::Entry>,
        disk: Vec<u8>,
        key: Vec<u8>,
    }

    impl EncryptedJournalFixture {
        fn new() -> Self {
            use molva_core::domain::journal::Journal;

            let directory = tempfile::tempdir().unwrap();
            let mut config = Config::default();
            config.journal.path = directory
                .path()
                .join("synthetic.jsonl")
                .display()
                .to_string();
            config.journal.key_path = directory.path().join("synthetic.key").display().to_string();
            config.journal.key_source = "file".into();
            config.journal.encrypt = true;
            config.journal.include_text = true;
            config.mcp.enabled = true;
            let config_path = directory.path().join("synthetic.toml");
            config.save_plain(&config_path).unwrap();
            assert!(
                Config::runtime_path(&config_path).unwrap().is_none(),
                "synthetic MCP tests must not read an external runtime configuration"
            );
            let ctx = Context {
                config: Config::load(&config_path).unwrap(),
                config_path,
                socket: directory.path().join("unused.sock"),
            };
            let entries = vec![
                Self::entry(1, "2020-01-01T12:00:00Z", "file", 4, 10),
                Self::entry(2, "2020-01-02T12:00:00Z", "mcp", 8, 20),
                Self::entry(3, "2020-01-03T12:00:00Z", "mic", 12, 30),
            ];
            let mut journal = FileJournal::open_for(&ctx.config).unwrap();
            assert!(journal.encrypts());
            for entry in &entries {
                journal.append(entry).unwrap();
            }
            let disk = std::fs::read(journal.path()).unwrap();
            let key = std::fs::read(ctx.config.journal_key_path().unwrap()).unwrap();
            Self {
                directory,
                ctx,
                entries,
                disk,
                key,
            }
        }

        fn entry(
            id: u128,
            ts: &str,
            source: &str,
            words: u32,
            audio_secs: u32,
        ) -> molva_core::domain::Entry {
            serde_json::from_value(json!({
                "schema": molva_core::domain::entry::SCHEMA_VERSION,
                "id": uuid::Uuid::from_u128(id),
                "session_id": uuid::Uuid::nil(),
                "ts": ts,
                "mode": "dictation",
                "source": source,
                "app": if id == 2 { "SyntheticPlayer" } else { "SyntheticEditor" },
                "language": "en",
                "words": words,
                "audio_secs": audio_secs,
                "style": "raw",
                "stt_engine": "synthetic",
                "stt_model": "synthetic",
                "llm_used": id == 2,
                "local_llm": id != 2,
                "dict_hits": 0,
                "latency_ms": { "stt": id * 50, "rules": 0, "total": id * 100 },
                "tokens": if id == 2 { json!({"prompt":120,"completion":30}) } else { Value::Null },
                "llm_cost_usd": if id == 2 { Some(0.125) } else { None },
                "llm_calls": if id == 2 { 2 } else { 0 },
                "llm_unpriced_calls": if id == 2 { 1 } else { 0 },
                "text_raw": format!("synthetic raw nebula {id}"),
                "text_final": format!("synthetic final nebula {id}"),
                "audio_path": format!("synthetic-{id}.wav"),
                "segments": [
                    { "start_ms":1250,"end_ms":2500,"text":format!("synthetic segment comet {id}") },
                    { "start_ms":3000,"end_ms":4500,"text":format!("synthetic segment meteor {id}") }
                ]
            }))
            .unwrap()
        }

        fn assert_unchanged(&self) {
            let path = self.ctx.config.journal_path().unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), self.disk);
            assert_eq!(
                std::fs::read(self.ctx.config.journal_key_path().unwrap()).unwrap(),
                self.key
            );
            assert!(!FileJournal::open_for(&self.ctx.config)
                .unwrap()
                .broken_path()
                .exists());
            assert!(!path.with_extension("jsonl.tmp").exists());
        }
    }

    #[test]
    fn encrypted_mcp_fixture_seals_all_text_fields_on_disk() {
        use molva_core::app::journal_crypto::is_encrypted;

        let fixture = EncryptedJournalFixture::new();
        let disk = std::str::from_utf8(&fixture.disk).unwrap();
        let stored: Vec<molva_core::domain::Entry> = disk
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(stored.len(), fixture.entries.len());
        for (sealed, original) in stored.iter().zip(&fixture.entries) {
            for (ciphertext, plaintext) in [
                (
                    sealed.text_raw.as_deref().unwrap(),
                    original.text_raw.as_deref().unwrap(),
                ),
                (
                    sealed.text_final.as_deref().unwrap(),
                    original.text_final.as_deref().unwrap(),
                ),
            ] {
                assert!(is_encrypted(ciphertext));
                assert!(!disk.contains(plaintext));
            }
            assert_eq!(sealed.segments.len(), original.segments.len());
            for (sealed, original) in sealed.segments.iter().zip(&original.segments) {
                assert!(is_encrypted(&sealed.text), "segment text is not encrypted");
                assert!(!disk.contains(&original.text));
                assert_eq!(
                    (sealed.start_ms, sealed.end_ms),
                    (original.start_ms, original.end_ms)
                );
            }
            assert_eq!(sealed.words, original.words);
            assert_eq!(sealed.source, original.source);
        }
        fixture.assert_unchanged();
    }

    #[test]
    fn encrypted_mcp_history_decrypts_filters_and_limits_without_disk_changes() {
        let fixture = EncryptedJournalFixture::new();
        let answer = call_in(&fixture.ctx, "history_list", &json!({"limit":0}));
        let mut expected = fixture.entries.clone();
        expected.reverse();
        assert_eq!(answer, serde_json::to_value(&expected).unwrap());
        let filtered = call_in(
            &fixture.ctx,
            "history_list",
            &json!({
                "query":"RAW NEBULA", "app":"syntheticplayer",
                "since":"2020-01-02T12:00:00Z", "until":"2020-01-02T12:00:00Z"
            }),
        );
        assert_eq!(filtered, json!([fixture.entries[1]]));
        for query in ["RAW NEBULA 2", "FINAL NEBULA 2"] {
            assert_eq!(
                call_in(&fixture.ctx, "history_list", &json!({"query":query})),
                json!([fixture.entries[1]])
            );
        }
        assert_eq!(
            call_in(
                &fixture.ctx,
                "history_list",
                &json!({"since":"2020-01-02T12:00:00Z","limit":0})
            ),
            json!([fixture.entries[2], fixture.entries[1]])
        );
        assert_eq!(
            call_in(
                &fixture.ctx,
                "history_list",
                &json!({"until":"2020-01-02T12:00:00Z","limit":0})
            ),
            json!([fixture.entries[1], fixture.entries[0]])
        );
        assert_eq!(
            call_in(&fixture.ctx, "history_list", &json!({"limit":1})),
            json!([fixture.entries[2]])
        );
        assert_eq!(
            call_in(
                &fixture.ctx,
                "history_list",
                &json!({"query":"absent asteroid"})
            ),
            json!([])
        );
        fixture.assert_unchanged();
    }

    #[test]
    fn encrypted_mcp_search_returns_segment_timecodes_and_whole_transcripts() {
        let fixture = EncryptedJournalFixture::new();
        let hits = call_in(
            &fixture.ctx,
            "search_transcripts",
            &json!({"query":" COMET ","limit":0}),
        );
        let hits = hits.as_array().unwrap();
        assert_eq!(hits.len(), 2);
        for (hit, entry) in hits.iter().zip([&fixture.entries[1], &fixture.entries[0]]) {
            assert_eq!(hit["entry_id"], entry.id.to_string());
            assert_eq!(hit["file"], entry.audio_path.as_deref().unwrap());
            assert_eq!(hit["text"], entry.segments[0].text);
            assert_eq!(hit["start_ms"], 1250);
            assert_eq!(hit["end_ms"], 2500);
            assert_eq!(hit["timecode"], "00:01.250 → 00:02.500");
            assert_eq!(hit["language"], "en");
        }
        let whole = call_in(
            &fixture.ctx,
            "search_transcripts",
            &json!({"query":"NEBULA","limit":1}),
        );
        assert_eq!(whole.as_array().unwrap().len(), 1);
        assert_eq!(whole[0]["entry_id"], fixture.entries[1].id.to_string());
        assert_eq!(
            whole[0]["text"],
            fixture.entries[1].text_final.as_deref().unwrap()
        );
        assert!(whole[0].get("start_ms").is_none());
        assert!(whole[0].get("timecode").is_none());
        assert_eq!(
            call_in(
                &fixture.ctx,
                "search_transcripts",
                &json!({"query":"absent asteroid"})
            ),
            json!([])
        );
        fixture.assert_unchanged();
    }

    #[test]
    fn encrypted_mcp_stats_preserve_metrics_without_disk_changes() {
        let fixture = EncryptedJournalFixture::new();
        let answer = call_in(&fixture.ctx, "stats_summary", &json!({"days":0}));
        assert_eq!(answer["total_words"], 24);
        assert_eq!(answer["avg_wpm_all"], 24.0);
        assert_eq!(answer["minutes_recorded"], 1.0);
        assert_eq!(answer["mcp_entries"], 1);
        assert_eq!(answer["mcp_words"], 8);
        assert_eq!(answer["latency_ms"]["stt"], 100);
        assert_eq!(answer["latency_ms"]["total"], 200);
        assert_eq!(
            answer["tokens"],
            json!({
                "prompt":120,"completion":30,"cost_usd":0.125,"calls":2,"unpriced_calls":1
            })
        );
        assert_eq!(answer["series"].as_array().unwrap().len(), 3);
        assert_eq!(answer["by_app"].as_array().unwrap().len(), 2);
        fixture.assert_unchanged();
    }

    #[test]
    fn encrypted_mcp_with_a_wrong_key_hides_text_but_keeps_statistics() {
        let fixture = EncryptedJournalFixture::new();
        let expected_stats = call_in(&fixture.ctx, "stats_summary", &json!({"days":0}));
        let mut ctx = Context {
            config: fixture.ctx.config.clone(),
            config_path: fixture.ctx.config_path.clone(),
            socket: fixture.ctx.socket.clone(),
        };
        ctx.config.journal.key_path = fixture
            .directory
            .path()
            .join("wrong.key")
            .display()
            .to_string();
        molva_core::app::journal_crypto::JournalCipher::from_key_file(
            &ctx.config.journal_key_path().unwrap(),
        )
        .unwrap();
        ctx.config.save_plain(&ctx.config_path).unwrap();
        let history = call_in(&ctx, "history_list", &json!({"limit":0}));
        assert_eq!(history.as_array().unwrap().len(), 3);
        for entry in history.as_array().unwrap() {
            assert!(entry.get("text_raw").is_none());
            assert!(entry.get("text_final").is_none());
            assert_eq!(entry["segments"][0]["text"], "");
            assert_eq!(entry["segments"][0]["start_ms"], 1250);
            assert_eq!(entry["segments"][1]["text"], "");
        }
        assert!(call_error(&ctx, "search_transcripts", &json!({"query":""})).contains("required"));
        for query in ["comet", "enc1:"] {
            assert_eq!(
                call_in(&ctx, "search_transcripts", &json!({"query":query})),
                json!([])
            );
        }
        assert_eq!(
            call_in(&ctx, "stats_summary", &json!({"days":0})),
            expected_stats
        );
        fixture.ctx.config.save_plain(&ctx.config_path).unwrap();
        assert_eq!(
            call_in(&ctx, "history_list", &json!({"limit":1})),
            json!([fixture.entries[2]])
        );
        fixture.assert_unchanged();
    }

    #[test]
    fn encrypted_mcp_concurrent_read_calls_keep_the_journal_and_key_unchanged() {
        let fixture = EncryptedJournalFixture::new();
        std::thread::scope(|scope| {
            let calls: Vec<_> = (0..4)
                .map(|_| {
                    let ctx = &fixture.ctx;
                    scope.spawn(move || {
                        let history = call_in(ctx, "history_list", &json!({"limit":0}));
                        assert_eq!(history.as_array().unwrap().len(), 3);
                        assert_eq!(history[0]["text_final"], "synthetic final nebula 3");
                        let hits = call_in(ctx, "search_transcripts", &json!({"query":"comet"}));
                        assert_eq!(hits.as_array().unwrap().len(), 2);
                        assert_eq!(hits[0]["text"], "synthetic segment comet 2");
                        let stats = call_in(ctx, "stats_summary", &json!({"days":0}));
                        assert_eq!(stats["total_words"], 24);
                        assert_eq!(stats["mcp_entries"], 1);
                    })
                })
                .collect();
            for call in calls {
                call.join().expect("a concurrent MCP call failed");
            }
        });
        fixture.assert_unchanged();
    }

    #[test]
    fn encrypted_mcp_long_lived_context_reloads_configuration_on_each_call() {
        let fixture = EncryptedJournalFixture::new();
        let mut before = fixture.ctx.config.clone();
        before.journal.encrypt = false;
        before.save_plain(&fixture.ctx.config_path).unwrap();
        let old = Context {
            config: Config::load(&fixture.ctx.config_path).unwrap(),
            config_path: fixture.ctx.config_path.clone(),
            socket: fixture.ctx.socket.clone(),
        };
        fixture
            .ctx
            .config
            .save_plain(&fixture.ctx.config_path)
            .unwrap();
        let fresh = Context {
            config: Config::load(&fixture.ctx.config_path).unwrap(),
            config_path: fixture.ctx.config_path.clone(),
            socket: fixture.ctx.socket.clone(),
        };
        assert!(!old.config.journal.encrypt);
        assert!(fresh.config.journal.encrypt);
        let history = call_in(&old, "history_list", &json!({"limit":0}));
        assert_eq!(history.as_array().unwrap().len(), 3);
        let mut expected = fixture.entries.clone();
        expected.reverse();
        assert_eq!(history, serde_json::to_value(expected).unwrap());
        let hits = call_in(&old, "search_transcripts", &json!({"query":"comet"}));
        assert_eq!(hits.as_array().unwrap().len(), 2);
        assert_eq!(hits[0]["text"], fixture.entries[1].segments[0].text);
        assert_eq!(
            call_in(&old, "stats_summary", &json!({"days":0})),
            call_in(&fresh, "stats_summary", &json!({"days":0}))
        );
        let restored = call_in(&fresh, "history_list", &json!({"limit":1}));
        assert_eq!(restored, json!([fixture.entries[2]]));
        assert!(!fixture.directory.path().join("unused.sock").exists());
        fixture.assert_unchanged();
    }

    #[test]
    fn tools_reload_the_journal_path_and_stats_settings() {
        let fixture = EncryptedJournalFixture::new();
        let mut config = fixture.ctx.config.clone();
        config.stats.typing_baseline_wpm = 10;
        config.save_plain(&fixture.ctx.config_path).unwrap();
        let stats = call_in(&fixture.ctx, "stats_summary", &json!({"days":0}));
        assert!(
            (stats["saved_minutes"].as_f64().unwrap() - 1.4).abs() < 1e-6,
            "{stats}"
        );
        config.journal.path = fixture
            .directory
            .path()
            .join("empty.jsonl")
            .display()
            .to_string();
        config.save_plain(&fixture.ctx.config_path).unwrap();
        assert_eq!(call_in(&fixture.ctx, "history_list", &json!({})), json!([]));
        assert_eq!(
            call_in(
                &fixture.ctx,
                "search_transcripts",
                &json!({"query":"comet"})
            ),
            json!([])
        );
        assert_eq!(
            call_in(&fixture.ctx, "stats_summary", &json!({"days":0}))["total_words"],
            0
        );
        fixture.assert_unchanged();
    }

    fn list_response(ctx: &Context) -> Value {
        handle(ctx, &json!({"jsonrpc":"2.0","id":9,"method":"tools/list"})).unwrap()
    }

    fn assert_tools_refused(ctx: &Context, secret: &str) {
        let listed = list_response(ctx);
        assert!(listed.get("error").is_some(), "{listed}");
        assert!(listed.get("result").is_none(), "{listed}");
        assert!(!listed.to_string().contains(secret));
        for name in tool_names() {
            let error = call_error(ctx, &name, &json!({"query":"voice","path":"unused.wav"}));
            assert!(!error.contains(secret), "{error}");
            assert!(!error.is_empty());
        }
    }

    #[test]
    fn tools_reload_the_enabled_flag_and_recover_after_reenable() {
        let ctx = context();
        assert_eq!(list_response(&ctx)["result"]["tools"], tools());
        let mut config = ctx.config.clone();
        config.mcp.enabled = false;
        config.save_plain(&ctx.config_path).unwrap();
        assert_tools_refused(&ctx, "synthetic-private-value");
        config.mcp.enabled = true;
        config.save_plain(&ctx.config_path).unwrap();
        assert_eq!(list_response(&ctx)["result"]["tools"], tools());
        assert!(call_in(&ctx, "search_docs", &json!({"query":"voice"}))["hits"].is_array());
    }

    #[test]
    fn invalid_and_missing_configuration_refuse_tools_without_secret_leaks_or_fallback() {
        let ctx = context();
        let secret = "synthetic-private-config-value";
        for invalid in [
            format!("[mcp]\nenabled = \"{secret}\"\n"),
            format!("[mcp]\nenabled = true\nsecret = \"{secret}"),
            format!("[mcp]\nenabled = true\n[log]\nlevel = \"{secret}\"\n"),
        ] {
            std::fs::write(&ctx.config_path, invalid).unwrap();
            assert_tools_refused(&ctx, secret);
            for method in ["initialize", "ping"] {
                let response = handle(&ctx, &json!({"id":1,"method":method})).unwrap();
                assert!(response.get("result").is_some(), "{response}");
            }
        }
        let moved = ctx.config_path.with_extension("saved");
        std::fs::rename(&ctx.config_path, &moved).unwrap();
        assert_tools_refused(&ctx, secret);
        std::fs::rename(moved, &ctx.config_path).unwrap();
        ctx.config.save_plain(&ctx.config_path).unwrap();
        assert_eq!(list_response(&ctx)["result"]["tools"], tools());
    }

    #[test]
    fn transcribe_checks_the_reloaded_model_before_contacting_the_daemon() {
        let ctx = context();
        let mut config = ctx.config.clone();
        config.stt.model = "synthetic-new-model".into();
        config.save_plain(&ctx.config_path).unwrap();
        let error = call_error(
            &ctx,
            "transcribe",
            &json!({
                "path":"/synthetic/nonexistent.wav", "model":ctx.config.stt.model
            }),
        );
        assert!(error.contains("settings"), "{error}");
        let error = call_error(
            &ctx,
            "transcribe",
            &json!({
                "path":"/synthetic/nonexistent.wav", "model":config.stt.model
            }),
        );
        assert!(error.contains("cannot read the path"), "{error}");
    }

    #[test]
    fn initialize_answers_with_the_protocol_and_server_name() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let response = handle(&context(), &request).expect("a response");
        assert_eq!(response["result"]["serverInfo"]["name"], "molva");
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn there_is_no_tool_that_writes_the_settings() {
        let names = tool_names();
        assert!(!names.iter().any(|name| name == "config_get"));
        assert!(!names.iter().any(|name| name == "config_validate"));
        assert!(!names.iter().any(|name| name == "stats_export_csv"));
        assert!(!names.iter().any(|name| name.contains("config")));
    }

    fn tool_names() -> Vec<String> {
        tools()
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect()
    }

    fn tool_schema(name: &str) -> Value {
        tools()
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == name)
            .expect("the tool")
            .clone()
    }

    /// Run one `tools/call` through the dispatcher and read the JSON it answered with
    fn call(name: &str, arguments: &Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let response = handle(&context(), &request).expect("a response");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("a text answer");
        let answer: Value = serde_json::from_str(text).expect("json");
        assert_eq!(response["result"]["isError"], false, "{answer}");
        answer
    }

    #[test]
    fn the_tool_list_still_names_every_earlier_tool_and_the_new_one() {
        let names = tool_names();
        for name in [
            "transcribe",
            "search_transcripts",
            "stats_summary",
            "history_list",
            "models_list",
            "search_docs",
        ] {
            assert!(names.iter().any(|tool| tool == name), "{name} is missing");
        }
    }

    #[test]
    fn the_search_docs_schema_bounds_the_answer() {
        let tool = tool_schema("search_docs");
        let schema = &tool["inputSchema"];
        assert_eq!(schema["required"][0], "query");
        assert_eq!(schema["properties"]["limit"]["minimum"], 1);
        assert_eq!(schema["properties"]["limit"]["maximum"], DOCS_LIMIT_MAX);
        assert_eq!(schema["properties"]["lang"]["enum"], json!(["en", "ru"]));
    }

    #[test]
    fn search_docs_answers_through_the_dispatcher() {
        let answer = call("search_docs", &json!({ "query": "voice", "limit": 3 }));
        let hits = answer["hits"].as_array().expect("a list of hits");
        assert!(hits.len() <= 3, "{answer}");
        // An empty index means this binary was built without the repository documents
        if answer["indexed"].as_u64().unwrap_or(0) > 0 {
            assert!(
                !hits.is_empty(),
                "a word of the documents found nothing: {answer}"
            );
            for hit in hits {
                for field in ["path", "title", "lang", "url", "score", "snippet"] {
                    assert!(!hit[field].is_null(), "{field} is missing in {hit}");
                }
                assert!(hit["url"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("https://"));
            }
        }
    }

    #[test]
    fn search_docs_keeps_one_language_when_asked() {
        let answer = call("search_docs", &json!({ "query": "голос", "lang": "ru" }));
        for hit in answer["hits"].as_array().expect("a list of hits") {
            assert_eq!(hit["lang"], "ru");
        }
        if answer["indexed"].as_u64().unwrap_or(0) > 0 {
            assert!(
                !answer["hits"].as_array().expect("hits").is_empty(),
                "{answer}"
            );
        }
    }

    #[test]
    fn search_docs_refuses_a_language_it_does_not_index() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": { "name": "search_docs", "arguments": { "query": "voice", "lang": "de" } }
        });
        let response = handle(&context(), &request).expect("a response");
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn a_notification_gets_no_response() {
        let request = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle(&context(), &request).is_none());
    }

    /// The model belongs to the daemon, so this file must not hold the direct path
    #[test]
    fn the_transcribe_tool_never_loads_a_model_of_its_own() {
        let source = include_str!("mcp.rs");
        // The needle is built at run time, so the test itself does not match the search
        let direct = concat!("transcribe", "_outcome");
        let engine_factory = concat!("build_stt", "_with");
        assert!(
            !source.contains(direct),
            "transcribe fell back to the direct model path"
        );
        assert!(
            !source.contains(engine_factory),
            "the MCP process builds an engine of its own"
        );
        assert!(
            source.contains("Command::FileTranscribe") && source.contains("ensure_running"),
            "transcribe does not go through the daemon"
        );
    }

    struct FakeDaemon {
        socket: PathBuf,
        seen: Arc<Mutex<Vec<FileTranscriptionRequest>>>,
        stopper: molva_core::infra::ipc::Stopper,
    }

    impl FakeDaemon {
        /// A daemon that answers ping and the file command, in a temporary directory
        fn start(
            answer: impl Fn(usize) -> Result<Value, ErrorCode> + Send + Sync + 'static,
        ) -> Self {
            use molva_core::infra::ipc::{RequestHandler, Server};
            use std::sync::atomic::{AtomicUsize, Ordering};

            struct Handler {
                seen: Arc<Mutex<Vec<FileTranscriptionRequest>>>,
                answer: Box<dyn Fn(usize) -> Result<Value, ErrorCode> + Send + Sync>,
                calls: AtomicUsize,
            }
            impl RequestHandler for Handler {
                fn handle(
                    &self,
                    cmd: Command,
                ) -> Result<Value, molva_core::ipc::protocol::IpcError> {
                    match cmd {
                        Command::Ping => Ok(json!({ "pid": 4242 })),
                        Command::FileTranscribe(request) => {
                            let index = self.calls.fetch_add(1, Ordering::SeqCst);
                            self.seen.lock().unwrap().push(request);
                            (self.answer)(index).map_err(|code| {
                                molva_core::ipc::protocol::IpcError {
                                    code,
                                    message: "the daemon is busy".into(),
                                    hint: Some("try again later".into()),
                                }
                            })
                        }
                        _ => Ok(Value::Null),
                    }
                }
                fn subscribe(
                    &self,
                    _levels: bool,
                ) -> Option<std::sync::mpsc::Receiver<molva_core::ipc::protocol::Event>>
                {
                    None
                }
            }

            let directory = tempfile::tempdir().unwrap();
            // The directory must outlive the call: keep it by leaking the path into the socket
            let socket = directory.keep().join("molva.sock");
            let server = Server::bind(&socket).unwrap();
            let stopper = server.stopper();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let handler: Arc<dyn RequestHandler> = Arc::new(Handler {
                seen: seen.clone(),
                answer: Box::new(answer),
                calls: AtomicUsize::new(0),
            });
            std::thread::spawn(move || {
                let _ = server.serve(&handler);
            });
            for _ in 0..200 {
                if Client::connect(&socket).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Self {
                socket,
                seen,
                stopper,
            }
        }

        fn seen(&self) -> Vec<FileTranscriptionRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Drop for FakeDaemon {
        fn drop(&mut self) {
            self.stopper.stop();
        }
    }

    fn report_json(file: &str) -> Value {
        json!({
            "results": [{
                "file": file,
                "text": "текст из демона",
                "audio_secs": 0.5,
                "latency_ms": 42,
                "language": "ru"
            }],
            "errors": []
        })
    }

    fn write_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..8_000 {
            writer.write_sample(1_000i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn transcribe_answers_from_the_daemon_and_marks_the_journal_source() {
        let daemon = FakeDaemon::start(|_| Ok(report_json("from the daemon")));
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("lecture.wav");
        write_wav(&wav);

        let answer = call_in(
            &context_at(&daemon.socket),
            "transcribe",
            &json!({ "path": wav.display().to_string(), "timecodes": true }),
        );
        assert_eq!(answer["results"][0]["text"], "текст из демона");
        assert_eq!(answer["errors"], json!([]));

        let seen = daemon.seen();
        assert_eq!(seen.len(), 1, "the daemon did not receive the file");
        assert_eq!(seen[0].path, wav.canonicalize().unwrap());
        assert!(seen[0].timecodes);
        assert_eq!(seen[0].source, Source::Mcp);
    }

    #[test]
    fn transcribe_keeps_one_configuration_snapshot_for_the_whole_batch() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = context();
        let mut disabled = fixture.config.clone();
        disabled.mcp.enabled = false;
        let config_path = fixture.config_path.clone();
        let daemon = FakeDaemon::start(move |_| {
            disabled.save_plain(&config_path).unwrap();
            Ok(report_json("snapshot batch"))
        });
        let ctx = Context {
            config: fixture.config.clone(),
            config_path: fixture.config_path.clone(),
            socket: daemon.socket.clone(),
        };
        write_wav(&directory.path().join("one.wav"));
        write_wav(&directory.path().join("two.wav"));
        let answer = call_in(&ctx, "transcribe", &json!({"path":directory.path()}));
        assert_eq!(answer["results"].as_array().unwrap().len(), 2);
        assert_eq!(daemon.seen().len(), 2);
        assert_tools_refused(&ctx, "synthetic-private-value");
        assert_eq!(daemon.seen().len(), 2);
    }

    #[test]
    fn transcribe_waits_out_a_busy_daemon_then_takes_the_answer() {
        let daemon = FakeDaemon::start(|index| {
            if index < 2 {
                Err(ErrorCode::Busy)
            } else {
                Ok(report_json("after the phrase"))
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("a.wav");
        write_wav(&wav);

        let mut client = Client::connect(&daemon.socket).unwrap();
        let report = file_request(
            &mut client,
            &FileTranscriptionRequest {
                path: wav,
                style: None,
                timecodes: true,
                no_llm: true,
                source: Source::Mcp,
                chunked: false,
            },
            Duration::from_secs(5),
        )
        .expect("the answer after two refusals");
        assert_eq!(report.results[0].file, "after the phrase");
        assert_eq!(daemon.seen().len(), 3);
    }

    #[test]
    fn a_daemon_that_stays_busy_gives_a_bounded_error() {
        let daemon = FakeDaemon::start(|_| Err(ErrorCode::Busy));
        let mut client = Client::connect(&daemon.socket).unwrap();
        let started = Instant::now();
        let err = file_request(
            &mut client,
            &FileTranscriptionRequest {
                path: PathBuf::from("/tmp/a.wav"),
                style: None,
                timecodes: true,
                no_llm: false,
                source: Source::Mcp,
                chunked: false,
            },
            Duration::from_millis(400),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "no bound");
        assert!(err.contains("busy"), "{err}");
        assert!(err.contains("try again later"), "{err}");
    }

    #[test]
    fn a_model_other_than_the_one_the_daemon_runs_is_refused() {
        // No daemon is started for this: the request never leaves the process
        let err = call_error(
            &context(),
            "transcribe",
            &json!({ "path": "/tmp/a.wav", "model": "large-v3" }),
        );
        assert!(err.contains("settings"), "{err}");
    }

    /// Run a `tools/call` through the dispatcher of one context and read the JSON answer
    fn call_in(ctx: &Context, name: &str, arguments: &Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let response = handle(ctx, &request).expect("a response");
        assert_eq!(response["result"]["isError"], false, "{response}");
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
            .expect("json")
    }

    /// The text of a failed `tools/call`
    fn call_error(ctx: &Context, name: &str, arguments: &Value) -> String {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let response = handle(ctx, &request).expect("a response");
        assert_eq!(response["result"]["isError"], true, "{response}");
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }
}
