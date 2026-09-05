// SPDX-License-Identifier: MIT
//! `molva stats` — сводка, ряды по дням, разбивка по приложениям и сессиям.

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
    /// Ширина окна в днях; `0` — всё время.
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
            Period::Today => "сегодня",
            Period::Week => "за неделю",
            Period::Month => "за месяц",
            Period::All => "за всё время",
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
    /// Что показать разбивкой: по дням, по сессиям или по приложениям
    #[arg(long, value_enum, default_value_t = Series::Day)]
    pub series: Series,
    /// Выдать сводку как JSON — ровно ту форму, что читает GUI
    #[arg(long)]
    pub json: bool,
    /// Сколько строк разбивки показать
    #[arg(long)]
    pub last: Option<usize>,
    #[command(subcommand)]
    pub action: Option<StatsAction>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum StatsAction {
    /// Выгрузить реплики в CSV
    Export { file: PathBuf },
    /// Начать статистику заново; история при этом сохраняется
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
            println!("Выгружено реплик: {} → {}", entries.len(), file.display());
            Ok(())
        }
        Some(StatsAction::Reset { yes }) => {
            if !confirm(
                &format!(
                    "Обнулить статистику ({} реплик)? История останется.",
                    entries.len()
                ),
                yes,
            )? {
                println!("Отменено.");
                return Ok(());
            }
            stats::write_reset_marker(&journal_path, Utc::now())?;
            println!("Статистика обнулена. История реплик на месте.");
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

/// Человекочитаемый отчёт: сводка, спарклайн и выбранная разбивка.
pub(crate) fn report(summary: &StatsSummary, entries: &[Entry], args: &StatsArgs) -> String {
    let mut out = String::new();
    out.push_str(&format!("Статистика {}\n\n", args.period.title()));
    out.push_str(&format!("  Слов сегодня      {}\n", summary.words_today));
    out.push_str(&format!("  Слов всего        {}\n", summary.total_words));
    out.push_str(&format!(
        "  Скорость сегодня  {}\n",
        optional_wpm(summary.avg_wpm_today)
    ));
    out.push_str(&format!(
        "  Скорость 7 дней   {}\n",
        optional_wpm(summary.avg_wpm_7d)
    ));
    out.push_str(&format!(
        "  Скорость всего    {}\n",
        optional_wpm(summary.avg_wpm_all)
    ));
    out.push_str(&format!(
        "  Личный рекорд     {}{}\n",
        optional_wpm(summary.record_wpm),
        summary
            .record_at
            .map(|at| format!(" ({})", at.format("%Y-%m-%d")))
            .unwrap_or_default()
    ));
    out.push_str(&format!("  Дней подряд       {}\n", summary.streak_days));
    out.push_str(&format!(
        "  Наговорено        {:.1} мин\n",
        summary.minutes_recorded
    ));
    out.push_str(&format!(
        "  Сэкономлено       {:.1} мин\n",
        summary.saved_minutes
    ));
    out.push_str(&format!(
        "  Задержка          stt {} мс, всего {} мс\n",
        summary.latency_ms.stt, summary.latency_ms.total
    ));
    out.push_str(&format!(
        "  Токены            prompt {}, completion {}\n",
        summary.tokens.prompt, summary.tokens.completion
    ));

    let speeds: Vec<f32> = summary
        .series
        .iter()
        .map(|day| day.avg_wpm.unwrap_or(0.0))
        .collect();
    if !speeds.is_empty() {
        out.push_str(&format!(
            "\n  Скорость по дням  {}\n",
            stats::sparkline(&speeds)
        ));
    }

    out.push('\n');
    match args.series {
        Series::Day => {
            out.push_str(&format!(
                "{:<12}  {:>6}  {:>6}  {:>9}  {:>10}\n",
                "ДЕНЬ", "РЕПЛИК", "СЛОВ", "СКОРОСТЬ", "ЗАДЕРЖКА"
            ));
            for day in tail(&summary.series, args.last) {
                out.push_str(&format!(
                    "{:<12}  {:>6}  {:>6}  {:>9}  {:>7} мс\n",
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
                "{:<20}  {:>6}  {:>6}  {:>9}\n",
                "ПРИЛОЖЕНИЕ", "РЕПЛИК", "СЛОВ", "СКОРОСТЬ"
            ));
            for app in tail(&summary.by_app, args.last) {
                out.push_str(&format!(
                    "{:<20}  {:>6}  {:>6}  {:>9}\n",
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
                "{:<17}  {:<17}  {:>6}  {:>6}  {:>9}\n",
                "НАЧАЛО", "КОНЕЦ", "РЕПЛИК", "СЛОВ", "СКОРОСТЬ"
            ));
            for session in tail(&sessions, args.last) {
                out.push_str(&format!(
                    "{:<17}  {:<17}  {:>6}  {:>6}  {:>9}\n",
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

/// Последние `last` элементов; без ограничения — все.
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
            error: None,
            text_raw: None,
            text_final: Some("текст".into()),
            audio_path: None,
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
        assert!(report.contains("Слов сегодня      20"), "{report}");
        assert!(report.contains("Слов всего        30"), "{report}");
        assert!(report.contains("Скорость по дням"), "{report}");
        assert!(report.contains("2026-09-05"), "{report}");
        assert!(report.contains("ДЕНЬ"), "{report}");
    }

    #[test]
    fn the_app_breakdown_is_listed_when_asked() {
        let entries = vec![
            entry("2026-09-05T10:00:00Z", 10, "kitty"),
            entry("2026-09-05T10:01:00Z", 40, "firefox"),
        ];
        let summary = stats::summary(&entries, now(), &StatsConfig::default(), 7);
        let report = report(&summary, &entries, &args(Series::App));
        assert!(report.contains("ПРИЛОЖЕНИЕ"), "{report}");
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
        assert!(report.contains("НАЧАЛО"), "{report}");
        assert_eq!(
            report.matches("2026-09-05 1").count(),
            4,
            "две сессии по два столбца времени:\n{report}"
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
        assert!(report.contains("Слов всего        0"), "{report}");
        assert!(report.contains("Личный рекорд     —"), "{report}");
    }
}
