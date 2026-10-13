// SPDX-License-Identifier: MIT
//! Subcommands of `molva`. Each one is a separate file, so that the tracks do not fight for one module
//!
//! The general output rule: data goes to stdout, everything else (progress, warnings,
//! error messages) goes to stderr. So `molva transcribe ... | wc -w` counts words,
//! not the progress bar. The common small things live here: opening the journal, parsing dates,
//! confirming dangerous actions, an error with an exit code
//!
//! Declarations go one per line: the file is shared by several tracks, and this gives fewer conflicts

// Printing to stdout is the job of the command layer: the data for which `molva` is started goes here
// The `print_stdout` rule stays in force for the core and the GUI, where stdout does not belong to them
#![allow(clippy::print_stdout)]

pub(crate) mod bench;
pub(crate) mod completions;
pub(crate) mod config;
pub(crate) mod daemon;
pub(crate) mod devices;
pub(crate) mod dictionary;
pub(crate) mod doctor;
pub(crate) mod history;
pub(crate) mod models;
pub(crate) mod record;
pub(crate) mod secret;
pub(crate) mod setup;
pub(crate) mod stats;
pub(crate) mod status;
pub(crate) mod styles;
pub(crate) mod test_inject;
pub(crate) mod transcribe;

use std::io::{IsTerminal, Write};

use anyhow::Context;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use molva_core::app::journal::FileJournal;
use molva_core::Config;
use thiserror::Error;

/// The separator of the identifier in the `plain` and `rofi` formats: the printable record separator sign
pub(crate) const ID_SEPARATOR: char = '␟';

/// How many characters of the text to show in the one-line formats
pub(crate) const LINE_TEXT_LIMIT: usize = 120;

/// An error of a subcommand together with the exit code that it must give
#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct CmdError {
    pub message: String,
    pub code: u8,
}

impl CmdError {
    /// Wrong arguments or an unknown name
    pub(crate) const BAD_ARGS: u8 = crate::exit::BAD_ARGS;
    /// The recognition engine did not build or crashed
    pub(crate) const ENGINE: u8 = crate::exit::ENGINE;
    /// A file could not be read or written
    pub(crate) const FILE: u8 = crate::exit::FILE;
    /// The daemon is not running or does not answer
    pub(crate) const NO_DAEMON: u8 = crate::exit::NO_DAEMON;
    /// The daemon is already running: a second copy is not needed
    pub(crate) const ALREADY_RUNNING: u8 = crate::exit::ALREADY_RUNNING;

    pub(crate) fn args(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::BAD_ARGS,
        }
    }

    pub(crate) fn engine(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::ENGINE,
        }
    }

    pub(crate) fn file(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::FILE,
        }
    }

    /// The daemon is unreachable: the command did what it could and reports this honestly
    pub(crate) fn no_daemon(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::NO_DAEMON,
        }
    }

    /// The daemon is already running: the second copy exits with a message and its own code
    pub(crate) fn already_running(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::ALREADY_RUNNING,
        }
    }
}

/// Whether to show the progress bar: only to a live terminal and only with human output
///
/// In a pipe and with `--json` the progress stays silent, otherwise it lands in the run log and in front of the jury
pub(crate) fn progress_enabled(machine_output: bool) -> bool {
    !machine_output && std::io::stderr().is_terminal()
}

/// Open the journal by the settings, and create it if needed
pub(crate) fn open_journal(config: &Config) -> anyhow::Result<FileJournal> {
    let path = config.journal_path()?;
    let journal =
        FileJournal::open_for(config).with_context(|| format!("journal {}", path.display()))?;
    Ok(journal)
}

/// The text in one line: line breaks become spaces
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Cut to `max` characters and add an ellipsis
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Parse the border of a period: `7d`, `24h`, `30m`, `2026-09-01` or a full RFC 3339 timestamp
///
/// For a bare date, `end_of_day` decides whether to take the start of the day or its end
pub(crate) fn parse_moment(
    value: &str,
    now: DateTime<Utc>,
    end_of_day: bool,
) -> anyhow::Result<DateTime<Utc>> {
    let value = value.trim();
    if let Some(rest) = value.strip_suffix('d') {
        if let Ok(days) = rest.parse::<i64>() {
            return Ok(now - Duration::days(days));
        }
    }
    if let Some(rest) = value.strip_suffix('h') {
        if let Ok(hours) = rest.parse::<i64>() {
            return Ok(now - Duration::hours(hours));
        }
    }
    if let Some(rest) = value.strip_suffix('m') {
        if let Ok(minutes) = rest.parse::<i64>() {
            return Ok(now - Duration::minutes(minutes));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let time = if end_of_day {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        if let Some(naive) = time {
            return Ok(Utc.from_utc_datetime(&naive));
        }
    }
    if let Ok(moment) = DateTime::parse_from_rfc3339(value) {
        return Ok(moment.with_timezone(&Utc));
    }
    anyhow::bail!(
        "cannot parse the date {value:?}: expected 7d, 24h, 2026-09-01 or an RFC 3339 timestamp"
    )
}

/// Ask for confirmation if it was not given with the `--yes` flag
pub(crate) fn confirm(question: &str, yes: bool) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }
    print!("{question} [y/N]: ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes" | "д" | "да"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_helpers_carry_their_exit_codes() {
        assert_eq!(CmdError::args("x").code, 2);
        assert_eq!(CmdError::engine("x").code, 5);
        assert_eq!(CmdError::file("x").code, 6);
        assert_eq!(CmdError::file("no such file").to_string(), "no such file");
    }

    #[test]
    fn machine_output_never_shows_progress() {
        assert!(!progress_enabled(true));
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn relative_periods_count_back_from_now() {
        assert_eq!(
            parse_moment("7d", now(), false).unwrap(),
            DateTime::parse_from_rfc3339("2026-08-29T12:00:00Z").unwrap()
        );
        assert_eq!(
            parse_moment("24h", now(), false).unwrap(),
            DateTime::parse_from_rfc3339("2026-09-04T12:00:00Z").unwrap()
        );
        assert_eq!(
            parse_moment("30m", now(), false).unwrap(),
            DateTime::parse_from_rfc3339("2026-09-05T11:30:00Z").unwrap()
        );
    }

    #[test]
    fn a_bare_date_takes_the_start_or_the_end_of_the_day() {
        assert_eq!(
            parse_moment("2026-09-01", now(), false).unwrap(),
            DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            parse_moment("2026-09-01", now(), true).unwrap(),
            DateTime::parse_from_rfc3339("2026-09-01T23:59:59Z").unwrap()
        );
    }

    #[test]
    fn a_full_timestamp_is_taken_as_is_and_junk_is_refused() {
        assert_eq!(
            parse_moment("2026-09-01T08:30:00Z", now(), false).unwrap(),
            DateTime::parse_from_rfc3339("2026-09-01T08:30:00Z").unwrap()
        );
        let err = parse_moment("yesterday", now(), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("yesterday"), "{err}");
        assert!(err.contains("2026-09-01"), "{err}");
    }

    #[test]
    fn long_text_is_shortened_and_flattened() {
        assert_eq!(one_line("первая\nвторая   строка"), "первая вторая строка");
        assert_eq!(truncate("привет", 10), "привет");
        assert_eq!(truncate("привет мир", 6), "приве…");
        assert_eq!(truncate("привет", 6).chars().count(), 6);
    }
}
