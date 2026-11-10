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
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(ctx, name, &arguments) {
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
    let config = Config::load(config_path).map_err(|e| CmdError::file(e.to_string()))?;
    if !config.mcp.enabled {
        return Err(CmdError::args(
            "the MCP server is disabled: set mcp.enabled = true in the settings",
        ));
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

    fn context() -> Context {
        context_at(Path::new("/tmp/molva-mcp-test.sock"))
    }

    fn context_at(socket: &Path) -> Context {
        Context {
            config: Config::default(),
            config_path: PathBuf::from("/tmp/molva-mcp-test.toml"),
            socket: socket.to_path_buf(),
        }
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
