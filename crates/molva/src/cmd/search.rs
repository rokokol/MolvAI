// SPDX-License-Identifier: MIT
//! `molva search` — find text in the transcriptions of audio and video files
//!
//! The command reads the same journal as the history and uses the same matching code as the
//! GUI. A hit shows the source file and, when the engine reported segments, the timecode of
//! the segment that holds the match.

use std::io::Write;

use molva_core::app::search;
use molva_core::Config;

use super::{one_line, open_journal, parse_moment, CmdError};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Text to find; empty prints the newest transcriptions
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,

    /// Maximum number of hits; 0 means no limit
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// Only transcriptions newer than this: 7d, 24h, 2026-09-01 or an RFC 3339 moment
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,

    /// Only transcriptions older than this
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,

    /// Machine output: a JSON array of hits
    #[arg(long)]
    pub json: bool,
}

/// The human form of one hit
pub(crate) fn render(hit: &search::Hit) -> String {
    let place = if hit.file.is_empty() {
        "file".to_string()
    } else {
        hit.file.clone()
    };
    let text = one_line(&hit.text);
    match &hit.timecode {
        Some(timecode) => format!("{place} [{timecode}] {text}"),
        None => format!("{place}: {text}"),
    }
}

pub(crate) fn run(args: &Args, config: &Config, stdout: &mut dyn Write) -> Result<(), CmdError> {
    let now = chrono::Utc::now();
    let since = args
        .since
        .as_deref()
        .map(|value| parse_moment(value, now, false))
        .transpose()
        .map_err(|e| CmdError::args(format!("--since: {e}")))?;
    let until = args
        .until
        .as_deref()
        .map(|value| parse_moment(value, now, true))
        .transpose()
        .map_err(|e| CmdError::args(format!("--until: {e}")))?;

    let journal = open_journal(config).map_err(|e| CmdError::file(e.to_string()))?;
    let entries = journal
        .load_all()
        .map_err(|e| CmdError::file(e.to_string()))?;
    let entries: Vec<_> = entries
        .into_iter()
        .filter(|entry| since.is_none_or(|since| entry.ts >= since))
        .filter(|entry| until.is_none_or(|until| entry.ts <= until))
        .collect();

    let hits = search::search(&entries, args.query.as_deref().unwrap_or(""), args.limit);
    if args.json {
        let body = serde_json::to_string_pretty(&hits)
            .map_err(|e| CmdError::file(format!("cannot serialize the hits: {e}")))?;
        writeln!(stdout, "{body}").map_err(|e| CmdError::file(e.to_string()))?;
        return Ok(());
    }
    if hits.is_empty() {
        eprintln!("no matches");
        return Ok(());
    }
    for hit in &hits {
        writeln!(stdout, "{}", render(hit)).map_err(|e| CmdError::file(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn a_hit_with_a_timecode_keeps_the_file_and_the_time() {
        let hit = search::Hit {
            entry_id: Uuid::nil(),
            file: "/tmp/lecture.mp4".into(),
            recorded_at: Utc::now(),
            text: "вторая часть\nлекции".into(),
            start_ms: Some(2000),
            end_ms: Some(4000),
            timecode: Some("00:02.000 → 00:04.000".into()),
            language: Some("ru".into()),
        };
        assert_eq!(
            render(&hit),
            "/tmp/lecture.mp4 [00:02.000 → 00:04.000] вторая часть лекции"
        );
    }
}
