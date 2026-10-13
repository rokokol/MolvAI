// SPDX-License-Identifier: MIT
//! Paste text into the active window
//!
//! No platform has one single method, so this module has several. `ChainInjector` tries them
//! in an order that depends on the session and the compositor. The last one in the chain is the
//! clipboard: it always works, even if the user must press Ctrl+V

pub mod chain;
pub mod clipboard;
pub mod enigo_inj;
pub mod wayland_tools;

#[cfg(target_os = "linux")]
pub mod uinput;

use crate::domain::inject::OutputMode;

pub use chain::{ChainInjector, ClipboardOnlyInjector};
pub use clipboard::ClipboardGuard;

/// Parse `output.mode` from the config; an unknown value is `auto`
pub fn parse_output_mode(value: &str) -> OutputMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "paste" => OutputMode::Paste,
        "type" => OutputMode::Type,
        "clipboard" => OutputMode::Clipboard,
        _ => OutputMode::Auto,
    }
}

/// Window classes of terminals: Ctrl+V does not paste in them, they need Ctrl+Shift+V
///
/// Only real X11 and Wayland window classes — those that `hyprctl activewindow`,
/// `xprop WM_CLASS` or `swaymsg -t get_tree` show. Names of programs that have no window of
/// their own (tmux, screen) do not belong here: their window class is a foreign one
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

/// Whether the window class looks like a terminal
///
/// The match is either exact or on the last segment of a reverse DNS name
/// (`org.wezfurlong.wezterm` → `wezterm`). A plain "ends with" test does not work here: a short
/// name such as `st` would then make every window a terminal if its class ends with these letters
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
        assert_eq!(parse_output_mode("telepathy"), OutputMode::Auto);
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
        // The classes come from real windows: hyprctl activewindow, xprop WM_CLASS, swaymsg get_tree
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
            assert!(is_terminal_class(class), "{class} is a terminal");
        }
    }

    #[test]
    fn a_class_that_merely_ends_with_a_terminal_name_is_not_a_terminal() {
        // The short `st` must not make a terminal of every window with these letters at the tail
        assert!(!is_terminal_class("gnome-text-editor"));
        assert!(!is_terminal_class("libreoffice-writer-st"));
        assert!(!is_terminal_class("code"));
        assert!(!is_terminal_class("org.gnome.Nautilus"));
    }
}
