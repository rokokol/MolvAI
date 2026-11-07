// SPDX-License-Identifier: MIT
//! `molva mcp` — a Model Context Protocol server over stdio
//!
//! One JSON-RPC request per line on stdin, one response per line on stdout. There is no port
//! and no listener: the MCP client starts this process itself, so the lifetime and the access
//! are the client's. Settings are read-only here; the tools that change them are deliberately
//! absent.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use molva_core::app::journal::{self, FileJournal};
use molva_core::app::models;
use molva_core::app::search;
use molva_core::app::stats;
use molva_core::domain::entry::Source;
use molva_core::Config;
use serde_json::{json, Value};

use super::{transcribe, CmdError};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// The server's view of one MCP session
struct Context {
    config: Config,
    config_path: PathBuf,
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
        "ping" => Ok(json!({})),
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
        "shutdown" => Ok(json!({})),
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

/// The tool catalogue. A settings writer is absent on purpose: on NixOS the config is a
/// read-only store file, and on other systems the user edits it where they can see it
fn tools() -> Value {
    json!([
        {
            "name": "transcribe",
            "description": "Transcribe an audio or video file locally. Returns the text, the segments with timecodes and the language. The result is written to the journal",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File or directory path" },
                    "model": { "type": "string", "description": "Model name from models.toml; the configured model is used when absent" },
                    "timecodes": { "type": "boolean", "default": true, "description": "Include segments with timecodes" },
                    "style": { "type": "string", "description": "Post-processing style identifier; the default style is used when absent" },
                    "no_llm": { "type": "boolean", "default": false, "description": "Do not call the language model for this run" }
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
            "description": "Words, speed, latency and cost-related counters for a period; MCP transcriptions are counted separately",
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
    let style = arguments
        .get("style")
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = arguments
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let args = transcribe::Args {
        input: vec![PathBuf::from(path)],
        language: None,
        model,
        engine: None,
        style,
        no_llm,
        no_inject: false,
        json: true,
        timecodes,
        copy: false,
        out: None,
        recursive: false,
        stdin_format: None,
    };
    let outcome = transcribe::transcribe_outcome(&args, &ctx.config, &ctx.config_path, Source::Mcp)
        .map_err(|e| e.message)?;
    Ok(json!({
        "results": outcome.results,
        "errors": outcome.errors.into_iter().map(|(file, reason)| json!({
            "file": file, "reason": reason
        })).collect::<Vec<_>>(),
    }))
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

/// The stdio loop: one request per line, one response per line, diagnostics on stderr
pub(crate) fn run(config_path: &Path) -> Result<(), CmdError> {
    let config = Config::load(config_path).map_err(|e| CmdError::file(e.to_string()))?;
    let ctx = Context {
        config,
        config_path: config_path.to_path_buf(),
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

    fn context() -> Context {
        Context {
            config: Config::default(),
            config_path: PathBuf::from("/tmp/molva-mcp-test.toml"),
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
        let tools = tools();
        let names: Vec<&str> = tools
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&"config_get"));
        assert!(!names.contains(&"config_validate"));
        assert!(!names.contains(&"stats_export_csv"));
        assert!(!names.iter().any(|name| name.contains("config")));
    }

    #[test]
    fn a_notification_gets_no_response() {
        let request = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle(&context(), &request).is_none());
    }
}
