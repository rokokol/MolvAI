// SPDX-License-Identifier: MIT
//! Вставка текста в активное окно.
//!
//! Единого способа нет ни на одной платформе, поэтому здесь их несколько, а `ChainInjector`
//! перебирает их в порядке, который зависит от сессии и композитора. Последний в цепочке —
//! буфер обмена: он работает всегда, пусть и требует от пользователя нажать Ctrl+V.

pub mod chain;
pub mod clipboard;
pub mod enigo_inj;
pub mod wayland_tools;

#[cfg(target_os = "linux")]
pub mod uinput;

use crate::domain::inject::OutputMode;

pub use chain::{ChainInjector, ClipboardOnlyInjector};
pub use clipboard::ClipboardGuard;

/// Разбор `output.mode` из конфига; незнакомое значение — это `auto`.
pub fn parse_output_mode(value: &str) -> OutputMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "paste" => OutputMode::Paste,
        "type" => OutputMode::Type,
        "clipboard" => OutputMode::Clipboard,
        _ => OutputMode::Auto,
    }
}

/// Классы окон терминалов: в них Ctrl+V не вставляет, нужен Ctrl+Shift+V.
const TERMINAL_CLASSES: &[&str] = &[
    "kitty",
    "alacritty",
    "foot",
    "footclient",
    "wezterm",
    "org.wezfurlong.wezterm",
    "konsole",
    "gnome-terminal",
    "xterm",
    "urxvt",
    "terminator",
    "tilix",
];

/// Похож ли класс окна на терминал.
pub fn is_terminal_class(class: &str) -> bool {
    let class = class.trim().to_ascii_lowercase();
    TERMINAL_CLASSES
        .iter()
        .any(|known| class == *known || class.ends_with(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mode_is_parsed_from_config_strings() {
        assert_eq!(parse_output_mode("paste"), OutputMode::Paste);
        assert_eq!(parse_output_mode(" TYPE "), OutputMode::Type);
        assert_eq!(parse_output_mode("clipboard"), OutputMode::Clipboard);
        assert_eq!(parse_output_mode("auto"), OutputMode::Auto);
    }

    #[test]
    fn unknown_output_mode_falls_back_to_auto_instead_of_failing() {
        assert_eq!(parse_output_mode("телепатия"), OutputMode::Auto);
        assert_eq!(parse_output_mode(""), OutputMode::Auto);
    }

    #[test]
    fn terminals_are_recognised_by_window_class() {
        assert!(is_terminal_class("kitty"));
        assert!(is_terminal_class("Alacritty"));
        assert!(is_terminal_class("org.wezfurlong.wezterm"));
        assert!(!is_terminal_class("firefox"));
        assert!(!is_terminal_class("org.telegram.desktop"));
    }
}
