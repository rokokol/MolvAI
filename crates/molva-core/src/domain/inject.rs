// SPDX-License-Identifier: MIT
//! Вставка текста в активное поле: режимы, отчёт и контракт.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Как доставить текст в активное окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// `Type` для коротких реплик, `Paste` для длинных — порог в конфиге.
    Auto,
    /// Буфер обмена → Ctrl+V / Cmd+V → восстановление буфера.
    Paste,
    /// Посимвольный набор эмуляцией клавиатуры.
    Type,
    /// Только положить в буфер, ничего не нажимать.
    Clipboard,
}

impl OutputMode {
    /// Разрешение `Auto` по длине текста в символах.
    pub fn resolve(self, text: &str, auto_type_max_chars: usize) -> OutputMode {
        match self {
            OutputMode::Auto if text.chars().count() <= auto_type_max_chars => OutputMode::Type,
            OutputMode::Auto => OutputMode::Paste,
            other => other,
        }
    }
}

/// Что реально произошло при вставке: метод попадает в журнал реплики.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InjectReport {
    /// Например `hyprctl-paste`, `wtype-type`, `enigo-type`, `clipboard-only`.
    pub method: String,
    /// Что пробовали до успеха, для диагностики.
    pub attempts: Vec<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InjectError {
    #[error("способ вставки недоступен: {0}")]
    Unavailable(String),
    #[error("вставка не удалась: {0}")]
    Failed(String),
    #[error("операция не поддерживается этим способом вставки")]
    Unsupported,
    #[error("нет доступа к буферу обмена: {0}")]
    ClipboardDenied(String),
    #[error("в тексте есть символы, которые нельзя набрать этим способом")]
    UnsupportedCharacters,
}

/// Доставка текста в активное окно.
pub trait TextInjector: Send {
    fn id(&self) -> &'static str;
    /// Доступен ли способ в текущем окружении (есть ли утилита, права, сессия).
    fn available(&self) -> bool;
    /// `mode` уже разрешён из `Auto` вызывающей стороной.
    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError>;
    /// Скопировать текущее выделение (Command Mode). По умолчанию не поддерживается.
    fn copy_selection(&mut self) -> Result<String, InjectError> {
        Err(InjectError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_picks_type_for_short_and_paste_for_long() {
        assert_eq!(OutputMode::Auto.resolve("короткий", 200), OutputMode::Type);
        let long = "а".repeat(201);
        assert_eq!(OutputMode::Auto.resolve(&long, 200), OutputMode::Paste);
    }

    #[test]
    fn auto_threshold_counts_characters_not_bytes() {
        // 200 кириллических букв = 400 байт, но ровно 200 символов → ещё `Type`
        let exactly = "я".repeat(200);
        assert_eq!(OutputMode::Auto.resolve(&exactly, 200), OutputMode::Type);
    }

    #[test]
    fn explicit_modes_are_unchanged() {
        assert_eq!(OutputMode::Paste.resolve("x", 0), OutputMode::Paste);
        assert_eq!(OutputMode::Clipboard.resolve("x", 0), OutputMode::Clipboard);
    }
}
