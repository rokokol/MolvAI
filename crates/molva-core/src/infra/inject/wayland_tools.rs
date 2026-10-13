// SPDX-License-Identifier: MIT
//! Paste through external Wayland utilities: `hyprctl`, `wtype`, `ydotool`
//!
//! None of them is universal: `hyprctl` exists only in Hyprland and can only send key
//! combinations, `wtype` needs the `virtual-keyboard` protocol (KDE and GNOME do not have it),
//! `ydotool` needs a daemon and permission for `/dev/uinput`. So they are placed in a chain, and
//! not chosen once and for all

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use crate::infra::inject::clipboard::{ClipboardGuard, SystemClipboard};

/// Which paste key combination the window needs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteShortcut {
    /// Ctrl+V — everywhere except terminals
    CtrlV,
    /// Ctrl+Shift+V — terminals
    CtrlShiftV,
}

/// Run a utility and require a zero exit code
fn run(program: &str, args: &[&str]) -> Result<(), InjectError> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| InjectError::Unavailable(format!("{program} did not start: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(InjectError::Failed(format!(
        "{program} {}: {}",
        args.join(" "),
        if stderr.is_empty() {
            "non-zero exit code".to_string()
        } else {
            stderr
        }
    )))
}

/// Common sequence for all "paste" methods: clipboard → key combination → clipboard back
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
        Err(error) => {
            // The key combination did not arrive — the text must stay in the clipboard, or it is
            // lost
            clipboard.keep();
            Err(error)
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

/// Paste by means of Hyprland itself: the compositor synthesizes the key combination for the
/// active window
#[derive(Debug)]
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

    /// Change the paste key combination: terminals need Ctrl+Shift+V
    pub fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        self.shortcut = shortcut;
    }

    /// Argument of `sendshortcut`: `MODIFIERS, KEY, window`. The key is set by code, not by
    /// keysym: with the Russian layout active, the keysym `V` is not found on the keyboard, and
    /// Hyprland silently sends nothing. The code `55` is the physical V key in any layout
    fn shortcut_arg(&self) -> &'static str {
        match self.shortcut {
            PasteShortcut::CtrlV => "CTRL, code:55, activewindow",
            PasteShortcut::CtrlShiftV => "CTRL SHIFT, code:55, activewindow",
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
            // hyprctl can only press keys: typing text and "only to the clipboard" do not belong
            // here
            return Err(InjectError::Unsupported);
        }
        let arg = self.shortcut_arg();
        paste_via_clipboard(&mut self.clipboard, text, || {
            run("hyprctl", &["dispatch", "sendshortcut", arg])?;
            // Hyprland leaves the modifier pressed after sendshortcut (issue #6407), and the client
            // handles the combination only when the modifier is released. So the "release" is
            // not a cleanup after the fact but a required step of the paste. The pause between
            // them was tested on 0.56.2: without it the compositor merges the events and the paste
            // does not happen
            std::thread::sleep(MODIFIER_RELEASE_DELAY);
            release_modifiers();
            Ok(())
        })?;
        Ok(InjectReport {
            method: "hyprctl-paste".into(),
            attempts: Vec::new(),
        })
    }

    /// Command mode: copy the selection with Ctrl+C through the compositor and take it
    /// from the clipboard. The clipboard is not restored on purpose — a plain Ctrl+C behaves the
    /// same way
    fn copy_selection(&mut self) -> Result<String, InjectError> {
        use crate::infra::inject::clipboard::{wl_clear, wl_paste};
        // The clipboard is cleared before Ctrl+C: otherwise, if the application was too slow to
        // put out the selection, the old clipboard content goes to the model — for example the
        // result of the previous edit — and each next command builds on the old text. If the
        // clear is not available, only content that differs from the old one counts as fresh
        let before = wl_paste();
        let cleared = wl_clear().is_ok();
        // The code 54 is the physical C key: the keysym `C` is not found with the Russian layout
        run(
            "hyprctl",
            &["dispatch", "sendshortcut", "CTRL, code:54, activewindow"],
        )?;
        std::thread::sleep(MODIFIER_RELEASE_DELAY);
        release_modifiers();
        // The application does not return the selection at once: we wait for a new one to appear
        // in the clipboard
        let deadline = std::time::Instant::now() + SELECTION_SETTLE_TIMEOUT;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(SELECTION_POLL_INTERVAL);
            let Some(selection) = wl_paste() else {
                continue;
            };
            if selection.trim().is_empty() {
                continue;
            }
            if cleared || before.as_deref() != Some(selection.as_str()) {
                return Ok(selection);
            }
        }
        Err(InjectError::Failed(
            "no selection: the clipboard did not change after Ctrl+C".into(),
        ))
    }
}

/// How long to wait for the application to put the selection in the clipboard after Ctrl+C
const SELECTION_SETTLE_TIMEOUT: Duration = Duration::from_millis(600);
/// Polling step of the clipboard during that time
const SELECTION_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Pause between the key combination and the release of the modifiers
const MODIFIER_RELEASE_DELAY: Duration = Duration::from_millis(80);

/// Release the modifiers that `sendshortcut` may have left pressed
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

/// Input synthesis through the `virtual-keyboard` protocol: the only way to *type* text on wlroots
#[derive(Debug)]
pub struct WtypeInjector {
    clipboard: ClipboardGuard<SystemClipboard>,
    shortcut: PasteShortcut,
    /// Pause between characters when typing, ms
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

    /// Change the paste key combination: terminals need Ctrl+Shift+V
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
                // `-k v` is a real key press; a bare `v` would be typed by wtype as text, and
                // the modifier does not apply to such "typing"
                paste_via_clipboard(&mut self.clipboard, text, || match shortcut {
                    PasteShortcut::CtrlV => run("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"]),
                    PasteShortcut::CtrlShiftV => run(
                        "wtype",
                        &[
                            "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl",
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

/// Input synthesis through `/dev/uinput` by a third-party daemon
#[derive(Debug)]
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

    /// Change the paste key combination: terminals need Ctrl+Shift+V
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
                // Codes from linux/input-event-codes.h: 29 ctrl, 42 shift, 47 v; `:1` is a press
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
        // The key by code, not by keysym: otherwise the combination does not arrive with the
        // Russian layout
        assert_eq!(plain.shortcut_arg(), "CTRL, code:55, activewindow");
        assert_eq!(terminal.shortcut_arg(), "CTRL SHIFT, code:55, activewindow");
    }

    #[test]
    fn missing_program_is_unavailable_not_a_panic() {
        let error = run("molva-no-such-program", &["--version"]).unwrap_err();
        assert!(matches!(error, InjectError::Unavailable(_)), "{error}");
    }

    #[test]
    fn nonzero_exit_is_a_failure_with_the_command_in_the_message() {
        let error = run("false", &[]).unwrap_err();
        assert!(matches!(error, InjectError::Failed(_)), "{error}");
        assert!(error.to_string().contains("false"), "{error}");
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
