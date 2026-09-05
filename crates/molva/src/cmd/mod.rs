// SPDX-License-Identifier: MIT
//! Подкоманды `molva`. Каждая — отдельный файл, чтобы дорожки не дрались за один модуль.
//!
//! Общее правило вывода: данные — в stdout, всё остальное (прогресс, предупреждения,
//! сообщения об ошибках) — в stderr. Поэтому `molva transcribe ... | wc -w` считает слова,
//! а не полоску прогресса.
//!
//! Объявления по одному на строку: файл общий для нескольких дорожек, так меньше конфликтов.

pub mod bench;
pub mod completions;
pub mod daemon;
pub mod doctor;
pub mod models;
pub mod record;
pub mod setup;
pub mod status;
pub mod test_inject;
pub mod transcribe;

use std::io::IsTerminal;

use thiserror::Error;

/// Ошибка подкоманды вместе с кодом выхода, который она должна дать.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct CmdError {
    pub message: String,
    pub code: u8,
}

impl CmdError {
    /// Неверные аргументы или неизвестное имя.
    pub const BAD_ARGS: u8 = crate::exit::BAD_ARGS;
    /// Движок распознавания не собрался или упал.
    pub const ENGINE: u8 = crate::exit::ENGINE;
    /// Не удалось прочитать или записать файл.
    pub const FILE: u8 = crate::exit::FILE;

    pub fn args(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::BAD_ARGS,
        }
    }

    pub fn engine(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::ENGINE,
        }
    }

    pub fn file(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Self::FILE,
        }
    }
}

/// Показывать ли полоску прогресса: только живому терминалу и только при человеческом выводе.
///
/// В пайпе и при `--json` прогресс молчит, иначе он попадёт в лог прогона и в глаза жюри.
pub fn progress_enabled(machine_output: bool) -> bool {
    !machine_output && std::io::stderr().is_terminal()
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
}
