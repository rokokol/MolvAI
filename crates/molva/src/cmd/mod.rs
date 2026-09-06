// SPDX-License-Identifier: MIT
//! Подкоманды `molva`. Каждая — отдельный файл, чтобы дорожки не дрались за один модуль.
//!
//! Общее правило вывода: данные — в stdout, всё остальное (прогресс, предупреждения,
//! сообщения об ошибках) — в stderr. Поэтому `molva transcribe ... | wc -w` считает слова,
//! а не полоску прогресса. Общие мелочи — открытие журнала, разбор дат, подтверждение опасных
//! действий, ошибка с кодом выхода — живут здесь.
//!
//! Объявления по одному на строку: файл общий для нескольких дорожек, так меньше конфликтов.

// Печать в stdout — это работа слоя команд: сюда идут данные, ради которых `molva` и запускают.
// Правило `print_stdout` остаётся в силе для ядра и GUI, где stdout принадлежит не им.
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

/// Разделитель идентификатора в форматах `plain` и `rofi`: печатный знак разделителя записей.
pub(crate) const ID_SEPARATOR: char = '␟';

/// Сколько символов текста показывать в однострочных форматах.
pub(crate) const LINE_TEXT_LIMIT: usize = 120;

/// Ошибка подкоманды вместе с кодом выхода, который она должна дать.
#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct CmdError {
    pub message: String,
    pub code: u8,
}

impl CmdError {
    /// Неверные аргументы или неизвестное имя.
    pub(crate) const BAD_ARGS: u8 = crate::exit::BAD_ARGS;
    /// Движок распознавания не собрался или упал.
    pub(crate) const ENGINE: u8 = crate::exit::ENGINE;
    /// Не удалось прочитать или записать файл.
    pub(crate) const FILE: u8 = crate::exit::FILE;
    /// Демон не запущен или не отвечает.
    pub(crate) const NO_DAEMON: u8 = crate::exit::NO_DAEMON;
    /// Демон уже запущен: вторая копия не нужна.
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

    /// Демон недоступен: команда сделала, что смогла, и честно об этом сообщает.
    pub(crate) fn no_daemon(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::NO_DAEMON,
        }
    }

    /// Демон уже запущен: вторая копия завершается с сообщением и своим кодом.
    pub(crate) fn already_running(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::ALREADY_RUNNING,
        }
    }
}

/// Показывать ли полоску прогресса: только живому терминалу и только при человеческом выводе.
///
/// В пайпе и при `--json` прогресс молчит, иначе он попадёт в лог прогона и в глаза жюри.
pub(crate) fn progress_enabled(machine_output: bool) -> bool {
    !machine_output && std::io::stderr().is_terminal()
}

/// Открыть журнал по настройкам, создав его при необходимости.
pub(crate) fn open_journal(config: &Config) -> anyhow::Result<FileJournal> {
    let path = config.journal_path()?;
    let journal =
        FileJournal::open_for(config).with_context(|| format!("журнал {}", path.display()))?;
    Ok(journal)
}

/// Текст в одну строку: переводы строк становятся пробелами.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Обрезать до `max` символов, добавив многоточие.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Разбор границы периода: `7d`, `24h`, `30m`, `2026-09-01` или полная метка RFC 3339.
///
/// Для голой даты `end_of_day` решает, брать начало дня или его конец.
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
    anyhow::bail!("не понимаю дату {value:?}: ожидается 7d, 24h, 2026-09-01 или метка RFC 3339")
}

/// Спросить подтверждение, если его не дали флагом `--yes`.
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
        assert_eq!(CmdError::file("нет файла").to_string(), "нет файла");
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
