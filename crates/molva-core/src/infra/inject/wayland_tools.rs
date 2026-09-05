// SPDX-License-Identifier: MIT
//! Вставка через внешние утилиты Wayland: `hyprctl`, `wtype`, `ydotool`.
//!
//! Ни один из них не универсален: `hyprctl` есть только в Hyprland и умеет лишь горячие клавиши,
//! `wtype` требует протокол `virtual-keyboard` (его нет в KDE и GNOME), `ydotool` — демона и прав
//! на `/dev/uinput`. Поэтому они выстраиваются в цепочку, а не выбираются раз и навсегда.

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use crate::infra::inject::clipboard::{ClipboardGuard, SystemClipboard};

/// Какое сочетание вставки нужно окну.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteShortcut {
    /// Ctrl+V — везде, кроме терминалов.
    CtrlV,
    /// Ctrl+Shift+V — терминалы.
    CtrlShiftV,
}

/// Запустить утилиту и потребовать нулевой код возврата.
fn run(program: &str, args: &[&str]) -> Result<(), InjectError> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| InjectError::Unavailable(format!("{program} не запустился: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(InjectError::Failed(format!(
        "{program} {}: {}",
        args.join(" "),
        if stderr.is_empty() {
            "ненулевой код возврата".to_string()
        } else {
            stderr
        }
    )))
}

/// Общая последовательность для всех «paste»-способов: буфер → сочетание клавиш → буфер назад.
fn paste_via_clipboard(
    clipboard: &mut ClipboardGuard<SystemClipboard>,
    text: &str,
    send: impl FnOnce() -> Result<(), InjectError>,
) -> Result<(), InjectError> {
    clipboard.stage(text)?;
    match send() {
        Ok(()) => {
            clipboard.restore()?;
            Ok(())
        }
        Err(err) => {
            // Сочетание не дошло — текст обязан остаться в буфере, иначе он потерян.
            clipboard.keep();
            Err(err)
        }
    }
}

fn guard(restore: bool, delay_ms: u32) -> ClipboardGuard<SystemClipboard> {
    ClipboardGuard::new(
        SystemClipboard::new(),
        restore,
        Duration::from_millis(u64::from(delay_ms)),
    )
}

/// Вставка средствами самого Hyprland: композитор синтезирует сочетание активному окну.
pub struct HyprctlInjector {
    clipboard: ClipboardGuard<SystemClipboard>,
    shortcut: PasteShortcut,
}

impl HyprctlInjector {
    pub fn new(restore_clipboard: bool, restore_delay_ms: u32, shortcut: PasteShortcut) -> Self {
        Self {
            clipboard: guard(restore_clipboard, restore_delay_ms),
            shortcut,
        }
    }

    pub fn is_available() -> bool {
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() && which::which("hyprctl").is_ok()
    }

    /// Сменить сочетание вставки: терминалам нужен Ctrl+Shift+V.
    pub fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        self.shortcut = shortcut;
    }

    /// Аргумент `sendshortcut`: `МОДИФИКАТОРЫ, КЛАВИША, окно`.
    fn shortcut_arg(&self) -> &'static str {
        match self.shortcut {
            PasteShortcut::CtrlV => "CTRL, V, activewindow",
            PasteShortcut::CtrlShiftV => "CTRL SHIFT, V, activewindow",
        }
    }
}

impl TextInjector for HyprctlInjector {
    fn id(&self) -> &'static str {
        "hyprctl"
    }

    fn available(&self) -> bool {
        Self::is_available()
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        if mode != OutputMode::Paste {
            // hyprctl умеет только нажимать клавиши: набор текста и «просто в буфер» — не сюда.
            return Err(InjectError::Unsupported);
        }
        let arg = self.shortcut_arg();
        paste_via_clipboard(&mut self.clipboard, text, || {
            run("hyprctl", &["dispatch", "sendshortcut", arg])?;
            // Hyprland 0.4x–0.5x иногда оставляет модификатор «нажатым» после sendshortcut
            // (issue #6407): следующий ввод пользователя уходит с Ctrl. Снимаем явно.
            release_modifiers();
            Ok(())
        })?;
        Ok(InjectReport {
            method: "hyprctl-paste".into(),
            attempts: Vec::new(),
        })
    }
}

/// Отпустить модификаторы, которые мог оставить нажатыми `sendshortcut`.
fn release_modifiers() {
    for mods in ["CTRL", "SHIFT"] {
        let _ = run(
            "hyprctl",
            &[
                "dispatch",
                "sendkeystate",
                &format!("{mods}, {mods}_L, up, activewindow"),
            ],
        );
    }
}

/// Синтез ввода через протокол `virtual-keyboard`: единственный способ *набрать* текст на wlroots.
pub struct WtypeInjector {
    clipboard: ClipboardGuard<SystemClipboard>,
    shortcut: PasteShortcut,
    /// Пауза между символами при наборе, мс.
    type_delay_ms: u32,
}

impl WtypeInjector {
    pub fn new(
        restore_clipboard: bool,
        restore_delay_ms: u32,
        shortcut: PasteShortcut,
        type_delay_ms: u32,
    ) -> Self {
        Self {
            clipboard: guard(restore_clipboard, restore_delay_ms),
            shortcut,
            type_delay_ms,
        }
    }

    pub fn is_available() -> bool {
        which::which("wtype").is_ok()
    }

    /// Сменить сочетание вставки: терминалам нужен Ctrl+Shift+V.
    pub fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        self.shortcut = shortcut;
    }
}

impl TextInjector for WtypeInjector {
    fn id(&self) -> &'static str {
        "wtype"
    }

    fn available(&self) -> bool {
        Self::is_available()
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        match mode {
            OutputMode::Type => {
                let delay = self.type_delay_ms.to_string();
                run("wtype", &["-d", &delay, "--", text])?;
                Ok(InjectReport {
                    method: "wtype-type".into(),
                    attempts: Vec::new(),
                })
            }
            OutputMode::Paste => {
                let shortcut = self.shortcut;
                paste_via_clipboard(&mut self.clipboard, text, || match shortcut {
                    PasteShortcut::CtrlV => run("wtype", &["-M", "ctrl", "v", "-m", "ctrl"]),
                    PasteShortcut::CtrlShiftV => run(
                        "wtype",
                        &[
                            "-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl",
                        ],
                    ),
                })?;
                Ok(InjectReport {
                    method: "wtype-paste".into(),
                    attempts: Vec::new(),
                })
            }
            _ => Err(InjectError::Unsupported),
        }
    }
}

/// Синтез ввода через `/dev/uinput` сторонним демоном.
pub struct YdotoolInjector {
    clipboard: ClipboardGuard<SystemClipboard>,
    shortcut: PasteShortcut,
}

impl YdotoolInjector {
    pub fn new(restore_clipboard: bool, restore_delay_ms: u32, shortcut: PasteShortcut) -> Self {
        Self {
            clipboard: guard(restore_clipboard, restore_delay_ms),
            shortcut,
        }
    }

    pub fn is_available() -> bool {
        which::which("ydotool").is_ok()
    }

    /// Сменить сочетание вставки: терминалам нужен Ctrl+Shift+V.
    pub fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        self.shortcut = shortcut;
    }
}

impl TextInjector for YdotoolInjector {
    fn id(&self) -> &'static str {
        "ydotool"
    }

    fn available(&self) -> bool {
        Self::is_available()
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        match mode {
            OutputMode::Type => {
                run("ydotool", &["type", "--", text])?;
                Ok(InjectReport {
                    method: "ydotool-type".into(),
                    attempts: Vec::new(),
                })
            }
            OutputMode::Paste => {
                // Коды из linux/input-event-codes.h: 29 ctrl, 42 shift, 47 v; `:1` — нажатие.
                let shortcut = self.shortcut;
                paste_via_clipboard(&mut self.clipboard, text, || match shortcut {
                    PasteShortcut::CtrlV => {
                        run("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"])
                    }
                    PasteShortcut::CtrlShiftV => run(
                        "ydotool",
                        &["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"],
                    ),
                })?;
                Ok(InjectReport {
                    method: "ydotool-paste".into(),
                    attempts: Vec::new(),
                })
            }
            _ => Err(InjectError::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprctl_only_pastes_and_refuses_to_type() {
        let mut injector = HyprctlInjector::new(true, 0, PasteShortcut::CtrlV);
        assert_eq!(
            injector.inject("текст", OutputMode::Type),
            Err(InjectError::Unsupported)
        );
        assert_eq!(
            injector.inject("текст", OutputMode::Clipboard),
            Err(InjectError::Unsupported)
        );
    }

    #[test]
    fn terminals_get_the_shift_variant_of_the_shortcut() {
        let plain = HyprctlInjector::new(true, 0, PasteShortcut::CtrlV);
        let terminal = HyprctlInjector::new(true, 0, PasteShortcut::CtrlShiftV);
        assert_eq!(plain.shortcut_arg(), "CTRL, V, activewindow");
        assert_eq!(terminal.shortcut_arg(), "CTRL SHIFT, V, activewindow");
    }

    #[test]
    fn missing_program_is_unavailable_not_a_panic() {
        let err = run("molva-no-such-program", &["--version"]).unwrap_err();
        assert!(matches!(err, InjectError::Unavailable(_)), "{err}");
    }

    #[test]
    fn nonzero_exit_is_a_failure_with_the_command_in_the_message() {
        let err = run("false", &[]).unwrap_err();
        assert!(matches!(err, InjectError::Failed(_)), "{err}");
        assert!(err.to_string().contains("false"), "{err}");
    }

    #[test]
    fn injector_ids_are_stable_for_the_journal() {
        assert_eq!(
            HyprctlInjector::new(true, 0, PasteShortcut::CtrlV).id(),
            "hyprctl"
        );
        assert_eq!(
            WtypeInjector::new(true, 0, PasteShortcut::CtrlV, 4).id(),
            "wtype"
        );
        assert_eq!(
            YdotoolInjector::new(true, 0, PasteShortcut::CtrlV).id(),
            "ydotool"
        );
    }
}
