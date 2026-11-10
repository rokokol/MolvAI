// SPDX-License-Identifier: MIT
//! `molva stats` — a summary, series by day, and a breakdown by application and by session

use std::path::PathBuf;

use chrono::Utc;
use clap::{Args, Subcommand, ValueEnum};
use molva_core::app::stats::{self, StatsSummary};
use molva_core::domain::entry::Entry;
use molva_core::Config;

use super::{confirm, open_journal, truncate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    /// The width of the window in days; `0` means all time
    pub(crate) fn range_days(self) -> u32 {
        match self {
            Period::Today => 1,
            Period::Week => 7,
            Period::Month => 30,
            Period::All => 0,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Period::Today => "for today",
            Period::Week => "for the week",
            Period::Month => "for the month",
            Period::All => "for all time",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Series {
    Day,
    Session,
    App,
}

#[derive(Debug, Args)]
pub(crate) struct StatsArgs {
    #[arg(long, value_enum, default_value_t = Period::Week)]
    pub period: Period,
    /// What to show as a breakdown: by day, by session or by application
    #[arg(long, value_enum, default_value_t = Series::Day)]
    pub series: Series,
    /// Print the summary as JSON — exactly the form that the GUI reads
    #[arg(long)]
    pub json: bool,
    /// How many rows of the breakdown to show
    #[arg(long)]
    pub last: Option<usize>,
    #[command(subcommand)]
    pub action: Option<StatsAction>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum StatsAction {
    /// Export the phrases to CSV
    Export { file: PathBuf },
    /// Start the statistics over; the history is kept
    Reset {
        #[arg(long)]
        yes: bool,
    },
}

pub(crate) fn run(args: StatsArgs, config: &Config) -> anyhow::Result<()> {
    let journal = open_journal(config)?;
    let journal_path = journal.path().to_path_buf();
    let all = journal.load_all()?;
    let reset_at = stats::read_reset_marker(&journal_path);
    let entries = stats::entries_since_reset(&all, reset_at);

    match args.action {
        Some(StatsAction::Export { file }) => {
            std::fs::write(&file, stats::export_csv(&entries))?;
            println!("Phrases exported: {} → {}", entries.len(), file.display());
            Ok(())
        }
        Some(StatsAction::Reset { yes }) => {
            if !confirm(
                &format!(
                    "Reset the statistics ({} phrases)? The history stays.",
                    entries.len()
                ),
                yes,
            )? {
                println!("Cancelled.");
                return Ok(());
            }
            stats::write_reset_marker(&journal_path, Utc::now())?;
            println!("Statistics reset. The phrase history is in place.");
            Ok(())
        }
        None => {
            let summary = stats::summary(
                &entries,
                Utc::now(),
                &config.stats,
                args.period.range_days(),
            );
            if args.json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                print!("{}", report(&summary, &entries, &args));
            }
            Ok(())
        }
    }
}

fn optional_wpm(value: Option<f32>) -> String {
    match value {
        Some(wpm) => format!("{wpm:.0} wpm"),
        None => "—".to_string(),
    }
}

/// A sum of money with enough decimals to keep a real charge from reading as zero
///
/// One phrase costs a fraction of a cent, so a fixed four decimals would print a day of
/// dictation as `$0.0000` and say nothing was spent
fn money(usd: f64) -> String {
    if usd >= 0.01 {
        format!("${usd:.2}")
    } else if usd >= 0.0001 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.6}")
    }
}

/// The charge of the model calls, with the calls that reported no charge named beside it
///
/// A sum of known charges is not the whole bill, so the count of silent calls travels with it.
/// A period with no cloud call at all says so instead of printing a zero that reads as a bill,
/// and so does a period where no call reported a charge: a zero there would be a guess
fn cost_line(tokens: &stats::TokenSummary) -> String {
    let total = tokens.cost_known + tokens.cost_unknown;
    if total == 0 {
        return "no cloud model calls".to_string();
    }
    if tokens.cost_known == 0 {
        let noun = if total == 1 { "call" } else { "calls" };
        return format!("unknown, the provider reported no charge for {total} {noun}");
    }
    let mut line = money(tokens.cost_usd);
    if tokens.cost_unknown > 0 {
        line.push_str(&format!(
            " ({} of {total} calls reported a charge)",
            tokens.cost_known
        ));
    }
    line
}

/// A report for a person: the summary, a sparkline and the chosen breakdown
pub(crate) fn report(summary: &StatsSummary, entries: &[Entry], args: &StatsArgs) -> String {
    let mut out = String::new();
    out.push_str(&format!("Statistics {}\n\n", args.period.title()));
    out.push_str(&format!("  Words today       {}\n", summary.words_today));
    out.push_str(&format!("  Words total       {}\n", summary.total_words));
    out.push_str(&format!(
        "  Speed today       {}\n",
        optional_wpm(summary.avg_wpm_today)
    ));
    out.push_str(&format!(
        "  Speed 7 days      {}\n",
        optional_wpm(summary.avg_wpm_7d)
    ));
    out.push_str(&format!(
        "  Speed overall     {}\n",
        optional_wpm(summary.avg_wpm_all)
    ));
    out.push_str(&format!(
        "  Personal record   {}{}\n",
        optional_wpm(summary.record_wpm),
        summary
            .record_at
            .map(|at| format!(" ({})", at.format("%Y-%m-%d")))
            .unwrap_or_default()
    ));
    out.push_str(&format!("  Days in a row     {}\n", summary.streak_days));
    out.push_str(&format!(
        "  Dictated          {:.1} min\n",
        summary.minutes_recorded
    ));
    out.push_str(&format!(
        "  Time saved        {:.1} min\n",
        summary.saved_minutes
    ));
    out.push_str(&format!(
        "  Latency           stt {} ms, total {} ms\n",
        summary.latency_ms.stt, summary.latency_ms.total
    ));
    out.push_str(&format!(
        "  Tokens            prompt {}, completion {}\n",
        summary.tokens.prompt, summary.tokens.completion
    ));
    out.push_str(&format!(
        "  Model cost        {}\n",
        cost_line(&summary.tokens)
    ));
    // The cost above is one sum over every source. This line says that a transcription through
    // MCP is inside it, so nobody has to take the sum on trust
    if summary.mcp_entries > 0 {
        let noun = if summary.mcp_entries == 1 {
            "file"
        } else {
            "files"
        };
        out.push_str(&format!(
            "  Of which MCP      {} {noun}, {} words\n",
            summary.mcp_entries, summary.mcp_words
        ));
    }

    let speeds: Vec<f32> = summary
        .series
        .iter()
        .map(|day| day.avg_wpm.unwrap_or(0.0))
        .collect();
    if !speeds.is_empty() {
        out.push_str(&format!(
            "\n  Speed by day      {}\n",
            stats::sparkline(&speeds)
        ));
    }

    out.push('\n');
    match args.series {
        Series::Day => {
            out.push_str(&format!(
                "{:<12}  {:>7}  {:>6}  {:>9}  {:>10}\n",
                "DAY", "PHRASES", "WORDS", "SPEED", "LATENCY"
            ));
            for day in tail(&summary.series, args.last) {
                out.push_str(&format!(
                    "{:<12}  {:>7}  {:>6}  {:>9}  {:>7} ms\n",
                    day.day,
                    day.entries,
                    day.words,
                    optional_wpm(day.avg_wpm),
                    day.avg_latency_ms
                ));
            }
        }
        Series::App => {
            out.push_str(&format!(
                "{:<20}  {:>7}  {:>6}  {:>9}\n",
                "APP", "PHRASES", "WORDS", "SPEED"
            ));
            for app in tail(&summary.by_app, args.last) {
                out.push_str(&format!(
                    "{:<20}  {:>7}  {:>6}  {:>9}\n",
                    truncate(&app.app, 20),
                    app.entries,
                    app.words,
                    optional_wpm(app.avg_wpm)
                ));
            }
        }
        Series::Session => {
            let sessions = stats::sessions(entries);
            out.push_str(&format!(
                "{:<17}  {:<17}  {:>7}  {:>6}  {:>9}\n",
                "START", "END", "PHRASES", "WORDS", "SPEED"
            ));
            for session in tail(&sessions, args.last) {
                out.push_str(&format!(
                    "{:<17}  {:<17}  {:>7}  {:>6}  {:>9}\n",
                    session.started.format("%Y-%m-%d %H:%M"),
                    session.ended.format("%Y-%m-%d %H:%M"),
                    session.entries,
                    session.words,
                    optional_wpm(session.avg_wpm)
                ));
            }
        }
    }
    out
}

/// The last `last` items; without a limit, all of them
fn tail<T>(items: &[T], last: Option<usize>) -> &[T] {
    match last {
        Some(count) if count < items.len() => &items[items.len() - count..],
        _ => items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::config::StatsConfig;
    use molva_core::domain::entry::{LatencyMs, Mode, Source};
    use uuid::Uuid;

    fn entry(ts: &str, words: u32, app: &str) -> Entry {
        Entry {
            schema: 1,
            id: Uuid::new_v4(),
            ts: chrono::DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::Mic,
            snippet: false,
            app: Some(app.into()),
            language: Some("ru".into()),
            audio_secs: 4.0,
            words,
            wpm: Entry::wpm_for(words, 4.0),
            style: "cleanup".into(),
            stt_engine: "fake".into(),
            stt_model: "fake".into(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method: None,
            latency_ms: LatencyMs {
                stt: 400,
                rules: 2,
                total: 500,
                ..Default::default()
            },
            tokens: None,
            llm_cost_usd: None,
            error: None,
            text_raw: None,
            text_final: Some("текст".into()),
            audio_path: None,
            segments: Vec::new(),
        }
    }

    fn args(series: Series) -> StatsArgs {
        StatsArgs {
            period: Period::Week,
            series,
            json: false,
            last: None,
            action: None,
        }
    }

    fn now() -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn periods_map_to_windows_in_days() {
        assert_eq!(Period::Today.range_days(), 1);
        assert_eq!(Period::Week.range_days(), 7);
        assert_eq!(Period::Month.range_days(), 30);
        assert_eq!(Period::All.range_days(), 0);
    }

    #[test]
    fn the_report_shows_the_numbers_and_a_sparkline() {
        let entries = vec![
            entry("2026-09-04T10:00:00Z", 10, "kitty"),
            entry("2026-09-05T10:00:00Z", 20, "firefox"),
        ];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(report.contains("Words today       20"), "{report}");
        assert!(report.contains("Words total       30"), "{report}");
        assert!(report.contains("Speed by day"), "{report}");
        assert!(report.contains("2026-09-05"), "{report}");
        assert!(report.contains("DAY"), "{report}");
    }

    #[test]
    fn the_app_breakdown_is_listed_when_asked() {
        let entries = vec![
            entry("2026-09-05T10:00:00Z", 10, "kitty"),
            entry("2026-09-05T10:01:00Z", 40, "firefox"),
        ];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::App));
        assert!(report.contains("APP"), "{report}");
        assert!(report.contains("firefox"), "{report}");
        assert!(report.contains("kitty"), "{report}");
    }

    #[test]
    fn the_session_breakdown_lists_starts_and_ends() {
        let entries = vec![
            entry("2026-09-05T10:00:00Z", 10, "kitty"),
            entry("2026-09-05T12:00:00Z", 10, "kitty"),
        ];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Session));
        assert!(report.contains("START"), "{report}");
        assert_eq!(
            report.matches("2026-09-05 1").count(),
            4,
            "two sessions with two time columns each:\n{report}"
        );
    }

    #[test]
    fn last_limits_the_breakdown_to_the_newest_rows() {
        let entries = vec![entry("2026-09-05T10:00:00Z", 10, "kitty")];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let mut args = args(Series::Day);
        args.last = Some(2);
        let report = report(&summary, &entries, &args);
        let rows = report
            .lines()
            .filter(|line| line.starts_with("2026-"))
            .count();
        assert_eq!(rows, 2, "{report}");
    }

    #[test]
    fn an_empty_history_still_prints_a_report() {
        let summary = stats::summary(&[], now(), &StatsConfig::default(), 7);
        let report = report(&summary, &[], &args(Series::Day));
        assert!(report.contains("Words total       0"), "{report}");
        assert!(report.contains("Personal record   —"), "{report}");
    }

    #[test]
    fn the_report_names_the_charge_of_the_model() {
        let mut first = entry("2026-09-05T10:00:00Z", 10, "kitty");
        first.llm_used = true;
        first.local_llm = false;
        first.llm_cost_usd = Some(0.000_004);
        let entries = vec![first];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(report.contains("Model cost        $0.000004"), "{report}");
    }

    #[test]
    fn a_silent_provider_is_named_beside_the_sum() {
        let mut known = entry("2026-09-05T10:00:00Z", 10, "kitty");
        known.llm_used = true;
        known.local_llm = false;
        known.llm_cost_usd = Some(0.000_004);
        let mut silent = entry("2026-09-05T10:01:00Z", 10, "kitty");
        silent.llm_used = true;
        silent.local_llm = false;
        let entries = vec![known, silent];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(
            report.contains("1 of 2 calls reported a charge"),
            "{report}"
        );
    }

    #[test]
    fn the_report_names_the_mcp_share_of_the_same_sum() {
        let mut through_mcp = entry("2026-09-05T10:00:00Z", 10, "kitty");
        through_mcp.source = Source::Mcp;
        through_mcp.llm_used = true;
        through_mcp.local_llm = false;
        through_mcp.llm_cost_usd = Some(0.000_004);
        let entries = vec![through_mcp];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(report.contains("Model cost        $0.000004"), "{report}");
        assert!(
            report.contains("Of which MCP      1 file, 10 words"),
            "{report}"
        );
    }

    #[test]
    fn a_period_without_mcp_says_nothing_about_it() {
        let entries = vec![entry("2026-09-05T10:00:00Z", 10, "kitty")];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(!report.contains("Of which MCP"), "{report}");
    }

    #[test]
    fn a_period_without_cloud_calls_says_so_instead_of_a_zero() {
        let entries = vec![entry("2026-09-05T10:00:00Z", 10, "kitty")];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(
            report.contains("Model cost        no cloud model calls"),
            "{report}"
        );
    }

    #[test]
    fn a_period_where_no_call_reported_a_charge_prints_no_sum() {
        let mut silent = entry("2026-09-05T10:00:00Z", 10, "kitty");
        silent.llm_used = true;
        silent.local_llm = false;
        let entries = vec![silent];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::Day));
        assert!(
            report.contains("unknown, the provider reported no charge for 1 call"),
            "{report}"
        );
        assert!(!report.contains("$0.0000"), "{report}");
    }
}
