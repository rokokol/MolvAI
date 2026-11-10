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
use crate::infra::inject::clipboard::{copy_selection_with, ClipboardGuard, SystemClipboard};

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

/// Reply of a successful `hyprctl dispatch`. Hyprland writes it to stdout, not stderr
const DISPATCH_OK: &str = "ok";

/// Run `hyprctl dispatch <arg>` and accept only the compositor's `ok`
///
/// The exit code alone decides nothing: an older Hyprland answers an unknown dispatcher with
/// `unknown request` and exit 0, and the Lua dispatcher of 0.56 answers a refused action with a
/// warning and exit 0. Both would pass as a paste that never happened. `Err` carries what the
/// compositor said, for the attempts of the chain
fn hyprctl_dispatch(program: &str, arg: &str) -> Result<(), String> {
    let output = Command::new(program)
        .args(["dispatch", arg])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{program} did not start: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reply = stdout.trim();
    if output.status.success() && reply == DISPATCH_OK {
        return Ok(());
    }
    if !reply.is_empty() {
        return Err(format!("{program} dispatch {arg}: {reply}"));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return Err(format!("{program} dispatch {arg}: {stderr}"));
    }
    Err(format!("{program} dispatch {arg}: non-zero exit code"))
}

/// Which `hyprctl dispatch` dialect the running compositor understands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    /// Hyprland 0.55 and later, where the config is Lua and the argument is a Lua expression
    Lua,
    /// An older Hyprland, or a hyprlang config: `dispatch <name> <arguments>`
    Legacy,
}

/// Send one shortcut in the dialect the compositor understands
///
/// The Lua form goes first and the old one is the fallback for an older compositor or a hyprlang
/// config. A refused attempt sends no keys, so the second one costs a process rather than a
/// doubled shortcut. The dialect comes back because the old form leaves the modifier pressed and
/// the Lua one does not
fn send_shortcut(program: &str, lua: &str, legacy_arg: &str) -> Result<Dialect, InjectError> {
    let lua_error = match hyprctl_dispatch(program, lua) {
        Ok(()) => return Ok(Dialect::Lua),
        Err(error) => error,
    };
    match hyprctl_dispatch(program, &format!("sendshortcut {legacy_arg}")) {
        Ok(()) => Ok(Dialect::Legacy),
        Err(legacy_error) => Err(InjectError::Failed(format!(
            "{lua_error}; the old form: {legacy_error}"
        ))),
    }
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

    /// The same shortcut in the Lua dialect of Hyprland 0.55 and later: a table, and no window
    /// field — without it the dispatcher sends to the focused window
    fn lua_arg(&self) -> &'static str {
        match self.shortcut {
            PasteShortcut::CtrlV => r#"hl.dsp.send_shortcut({mods = "CTRL", key = "code:55"})"#,
            PasteShortcut::CtrlShiftV => {
                r#"hl.dsp.send_shortcut({mods = "CTRL SHIFT", key = "code:55"})"#
            }
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
        let lua = self.lua_arg();
        paste_via_clipboard(&mut self.clipboard, text, || {
            if send_shortcut("hyprctl", lua, arg)? == Dialect::Legacy {
                // The old form leaves the modifier pressed (issue #6407), and the client handles
                // the combination only when the modifier is released. So the "release" is not a
                // cleanup after the fact but a required step of the paste. The pause between them
                // was tested on 0.56.2: without it the compositor merges the events and the paste
                // does not happen. The Lua form releases the modifier itself
                std::thread::sleep(MODIFIER_RELEASE_DELAY);
                release_legacy_modifiers();
            }
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
        copy_selection_with(|| {
            if send_shortcut("hyprctl", LUA_COPY, LEGACY_COPY)? == Dialect::Legacy {
                std::thread::sleep(MODIFIER_RELEASE_DELAY);
                release_legacy_modifiers();
            }
            Ok(())
        })
    }
}

/// The copy shortcut in the Lua dialect of Hyprland 0.55 and later. The code `54` is the
/// physical C key: the keysym `C` is not found with the Russian layout
const LUA_COPY: &str = r#"hl.dsp.send_shortcut({mods = "CTRL", key = "code:54"})"#;
/// The same shortcut in the old dialect: `sendshortcut MODIFIERS, KEY, window`
const LEGACY_COPY: &str = "CTRL, code:54, activewindow";

/// Pause between the key combination and the release of the modifiers
const MODIFIER_RELEASE_DELAY: Duration = Duration::from_millis(80);

/// Release the modifiers that the old `sendshortcut` may have left pressed
fn release_legacy_modifiers() {
    for mods in ["CTRL", "SHIFT"] {
        let _ = hyprctl_dispatch(
            "hyprctl",
            &format!("sendkeystate {mods}, {mods}_L, up, activewindow"),
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
    fn the_lua_dialect_names_the_same_keys_and_modifiers() {
        let plain = HyprctlInjector::new(true, 0, PasteShortcut::CtrlV);
        let terminal = HyprctlInjector::new(true, 0, PasteShortcut::CtrlShiftV);
        assert_eq!(
            plain.lua_arg(),
            r#"hl.dsp.send_shortcut({mods = "CTRL", key = "code:55"})"#
        );
        assert_eq!(
            terminal.lua_arg(),
            r#"hl.dsp.send_shortcut({mods = "CTRL SHIFT", key = "code:55"})"#
        );
        // The copy shortcut has its own constants; the key code is the physical C in both
        assert!(LUA_COPY.contains("code:54"), "{LUA_COPY}");
        assert!(LEGACY_COPY.contains("code:54"), "{LEGACY_COPY}");
    }

    #[test]
    fn a_missing_hyprctl_is_reported_not_a_panic() {
        let error = send_shortcut("/nonexistent/hyprctl", LUA_COPY, LEGACY_COPY).unwrap_err();
        assert!(error.to_string().contains("did not start"), "{error}");
    }

    /// A `hyprctl` on the disk that answers like one of the two compositors. The real command
    /// writes its reply to stdout, so the fake does the same
    #[cfg(unix)]
    mod fake_hyprctl {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);

        fn script(dir: &Path, lua_reply: &str, legacy_reply: &str) -> PathBuf {
            // Parallel tests must never name the same executable: a test that opens its script
            // while another one still writes it gets ETXTBSY from execve
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let name = format!("hyprctl-{}-{unique}", std::process::id());
            let path = dir.join(&name);
            let staged = dir.join(format!("{name}.tmp"));
            let text = format!(
                "#!/bin/sh\ncase \"$2\" in\n  hl.dsp.*) echo '{lua_reply}' ;;\n  *) echo '{legacy_reply}' ;;\nesac\n"
            );
            // The rename leaves no writer behind when execve starts the interpreter
            std::fs::write(&staged, text).expect("the fake hyprctl is written");
            let mut mode = std::fs::metadata(&staged)
                .expect("the fake exists")
                .permissions();
            mode.set_mode(0o755);
            std::fs::set_permissions(&staged, mode).expect("the fake is executable");
            std::fs::rename(&staged, &path).expect("the fake hyprctl is named");
            path
        }

        fn run_fake(lua_reply: &str, legacy_reply: &str) -> Result<Dialect, InjectError> {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let program = script(dir.path(), lua_reply, legacy_reply);
            send_shortcut(
                program.to_str().expect("the path is UTF-8"),
                LUA_COPY,
                LEGACY_COPY,
            )
        }

        #[test]
        fn a_lua_compositor_takes_the_lua_form() {
            assert_eq!(run_fake("ok", "unknown request").unwrap(), Dialect::Lua);
        }

        #[test]
        fn an_old_compositor_gets_the_old_form() {
            assert_eq!(run_fake("unknown request", "ok").unwrap(), Dialect::Legacy);
        }

        #[test]
        fn an_unknown_request_with_exit_zero_is_not_a_success() {
            // The trap of an old Hyprland: it answers "unknown request" and still exits 0
            let error = run_fake("unknown request", "unknown request").unwrap_err();
            assert!(matches!(error, InjectError::Failed(_)), "{error}");
            assert!(error.to_string().contains("unknown request"), "{error}");
        }

        #[test]
        fn a_warning_with_exit_zero_is_not_a_success() {
            // The Lua dispatcher of 0.56 prints a warning and exits 0 when it refuses
            let error =
                run_fake("warning: window not found", "warning: window not found").unwrap_err();
            assert!(error.to_string().contains("window not found"), "{error}");
        }
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
