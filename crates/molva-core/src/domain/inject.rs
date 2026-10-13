// SPDX-License-Identifier: MIT
//! Paste text into the active field: the modes, the report and the contract

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How to deliver text to the active window
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// `Type` for short phrases, `Paste` for long ones — the threshold is in the config
    Auto,
    /// Clipboard → Ctrl+V / Cmd+V → restore the clipboard
    Paste,
    /// Type character by character with keyboard emulation
    Type,
    /// Only put the text in the clipboard, press no keys
    Clipboard,
}

impl OutputMode {
    /// Resolve `Auto` by the length of the text in characters
    #[must_use]
    pub fn resolve(self, text: &str, auto_type_max_chars: usize) -> OutputMode {
        match self {
            OutputMode::Auto if text.chars().count() <= auto_type_max_chars => OutputMode::Type,
            OutputMode::Auto => OutputMode::Paste,
            other => other,
        }
    }
}

/// What really happened during the paste: the method goes to the journal entry of the phrase
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InjectReport {
    /// For example `hyprctl-paste`, `wtype-type`, `enigo-type`, `clipboard-only`
    pub method: String,
    /// What was tried before the success, for diagnostics
    pub attempts: Vec<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InjectError {
    #[error("paste method is unavailable: {0}")]
    Unavailable(String),
    #[error("paste failed: {0}")]
    Failed(String),
    #[error("this paste method does not support the operation")]
    Unsupported,
    #[error("no access to the clipboard: {0}")]
    ClipboardDenied(String),
    #[error("the text has characters that this method cannot type")]
    UnsupportedCharacters,
}

/// Delivery of text to the active window
pub trait TextInjector: std::fmt::Debug + Send {
    fn id(&self) -> &'static str;
    /// Whether the method is available in the current environment (utility, permissions, session)
    fn available(&self) -> bool;
    /// The caller has already resolved `mode` from `Auto`
    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError>;
    /// Copy the current selection (Command Mode). Not supported by default
    fn copy_selection(&mut self) -> Result<String, InjectError> {
        Err(InjectError::Unsupported)
    }
    /// Class of the window that gets the next paste
    ///
    /// It is needed where the paste method depends on the application: Ctrl+V pastes nothing in a
    /// terminal, which needs Ctrl+Shift+V. Implementations that do not care about the window do nothing
    fn set_window(&mut self, _class: Option<&str>) {}
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
        // 200 Cyrillic letters = 400 bytes, but exactly 200 characters → still `Type`
        let exactly = "я".repeat(200);
        assert_eq!(OutputMode::Auto.resolve(&exactly, 200), OutputMode::Type);
    }

    #[test]
    fn explicit_modes_are_unchanged() {
        assert_eq!(OutputMode::Paste.resolve("x", 0), OutputMode::Paste);
        assert_eq!(OutputMode::Clipboard.resolve("x", 0), OutputMode::Clipboard);
    }
}
