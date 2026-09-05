// SPDX-License-Identifier: MIT
//! Подкоманды `molva`: история, статистика, стили, словарь, настройки.
//!
//! Каждый модуль отвечает за разбор своих аргументов и печать результата; общие мелочи —
//! открытие журнала, разбор дат и подтверждение опасных действий — живут здесь.

pub mod config;
pub mod dictionary;
pub mod history;
pub mod stats;
pub mod styles;

use std::io::Write;

use anyhow::Context;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use molva_core::app::journal::FileJournal;
use molva_core::Config;

/// Разделитель идентификатора в форматах `plain` и `rofi`: печатный знак разделителя записей.
pub const ID_SEPARATOR: char = '␟';

/// Сколько символов текста показывать в однострочных форматах.
pub const LINE_TEXT_LIMIT: usize = 120;

/// Открыть журнал по настройкам, создав его при необходимости.
pub fn open_journal(config: &Config) -> anyhow::Result<FileJournal> {
    let path = config.journal_path()?;
    let journal = FileJournal::open_with(&path, config.journal.include_text)
        .with_context(|| format!("журнал {}", path.display()))?;
    Ok(journal)
}

/// Текст в одну строку: переводы строк становятся пробелами.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Обрезать до `max` символов, добавив многоточие.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Разбор границы периода: `7d`, `24h`, `30m`, `2026-09-01` или полная метка RFC 3339.
///
/// Для голой даты `end_of_day` решает, брать начало дня или его конец.
pub fn parse_moment(
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
    anyhow::bail!("не понимаю дату {value:?}: ожидается 7d, 24h, 2026-09-01 или метка RFC 3339")
}

/// Спросить подтверждение, если его не дали флагом `--yes`.
pub fn confirm(question: &str, yes: bool) -> anyhow::Result<bool> {
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
        let err = parse_moment("вчера", now(), false).unwrap_err().to_string();
        assert!(err.contains("вчера"), "{err}");
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
