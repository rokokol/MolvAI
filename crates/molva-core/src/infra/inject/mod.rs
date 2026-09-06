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
///
/// Только реальные классы окон X11 и Wayland — те, что показывает `hyprctl activewindow`,
/// `xprop WM_CLASS` или `swaymsg -t get_tree`. Имена программ, у которых своего окна нет
/// (tmux, screen), сюда не годятся: класс окна у них чужой.
const TERMINAL_CLASSES: &[&str] = &[
    "kitty",
    "alacritty",
    "foot",
    "footclient",
    "wezterm",
    "org.wezfurlong.wezterm",
    "ghostty",
    "com.mitchellh.ghostty",
    "konsole",
    "org.kde.konsole",
    "yakuake",
    "org.kde.yakuake",
    "gnome-terminal",
    "org.gnome.terminal",
    "org.gnome.console",
    "kgx",
    "xfce4-terminal",
    "lxterminal",
    "qterminal",
    "xterm",
    "uxterm",
    "urxvt",
    "rxvt",
    "st",
    "terminator",
    "tilix",
    "guake",
    "contour",
    "rio",
    "hyper",
    "com.raggesilver.blackbox",
];

/// Похож ли класс окна на терминал.
///
/// Совпадение либо точное, либо по последнему сегменту обратного DNS
/// (`org.wezfurlong.wezterm` → `wezterm`). Просто «оканчивается на» здесь не годится: короткое
/// имя вроде `st` тогда сделало бы терминалом каждое окно, чей класс кончается на эти буквы.
pub fn is_terminal_class(class: &str) -> bool {
    let class = class.trim().to_ascii_lowercase();
    let tail = class.rsplit('.').next().unwrap_or(&class);
    TERMINAL_CLASSES
        .iter()
        .any(|known| class == *known || tail == *known)
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

    #[test]
    fn the_terminals_people_actually_use_are_in_the_list() {
        // Классы взяты у настоящих окон: hyprctl activewindow, xprop WM_CLASS, swaymsg get_tree.
        for class in [
            "com.mitchellh.ghostty",
            "ghostty",
            "org.kde.konsole",
            "konsole",
            "foot",
            "footclient",
            "st",
            "xterm",
            "tilix",
            "terminator",
            "org.gnome.Terminal",
            "kgx",
            "xfce4-terminal",
        ] {
            assert!(is_terminal_class(class), "{class} — терминал");
        }
    }

    #[test]
    fn a_class_that_merely_ends_with_a_terminal_name_is_not_a_terminal() {
        // Короткое `st` не должно превращать в терминал каждое окно с такими буквами в хвосте.
        assert!(!is_terminal_class("gnome-text-editor"));
        assert!(!is_terminal_class("libreoffice-writer-st"));
        assert!(!is_terminal_class("code"));
        assert!(!is_terminal_class("org.gnome.Nautilus"));
    }
}
