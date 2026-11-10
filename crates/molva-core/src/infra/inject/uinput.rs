// SPDX-License-Identifier: MIT
//! Paste through an own virtual device, `/dev/uinput`
//!
//! This is the only method that depends neither on the compositor nor on third-party daemons, but
//! it needs permission for `/dev/uinput` (usually a udev rule for the `input` group). The device
//! does not know the keyboard layout: key codes go to the kernel, and the compositor applies the
//! layout, so only ASCII on the US layout can be typed blindly. Everything else goes through the
//! clipboard

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use serde_json::Value;

use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use crate::infra::inject::clipboard::{copy_selection_with, ClipboardGuard, SystemClipboard};
use crate::infra::inject::wayland_tools::PasteShortcut;
use crate::infra::platform::{self, Compositor, Platform};

const UINPUT_PATH: &str = "/dev/uinput";

/// The keyboard layout that the virtual device has to wear around a paste
///
/// A compositor may give the client the layout group of the keyboard that pressed the modifier.
/// The virtual device starts on group 0, so a paste can publish the US layout and leave it
/// active. The guard puts the compositor on group 0 for the shortcut and back on the user's
/// group afterwards. Each desktop is handled by the command its own session exposes; an
/// unsupported session simply has no guard
#[derive(Debug, Clone, PartialEq, Eq)]
enum LayoutTarget {
    Hyprland {
        device: String,
        physical: String,
        restore: u32,
    },
    Sway {
        device: String,
        physical: String,
        restore: u32,
    },
    Kde {
        restore: u32,
    },
    Gnome {
        restore: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayoutGuard {
    target: LayoutTarget,
}

impl LayoutGuard {
    fn prepare() -> Option<Self> {
        let target = match platform::detect() {
            Platform::Wayland(Compositor::Hyprland) => hyprland_target()?,
            Platform::Wayland(Compositor::Sway) => sway_target()?,
            Platform::Wayland(Compositor::Kde) => kde_target()?,
            Platform::Wayland(Compositor::Gnome) => gnome_target()?,
            _ => return None,
        };
        Some(Self { target })
    }

    fn before(&self) {
        match &self.target {
            LayoutTarget::Hyprland { device, .. } => switch_hyprland(device, 0),
            LayoutTarget::Sway { device, .. } => switch_sway(device, 0),
            LayoutTarget::Kde { .. } => switch_kde(0),
            LayoutTarget::Gnome { .. } => switch_gnome(0),
        }
    }

    fn after(&self) {
        match &self.target {
            LayoutTarget::Hyprland {
                device,
                physical,
                restore,
            } => {
                // The compositor can move the real keyboard too when the virtual one owns the
                // modifier; putting both back is what makes the next typed letter correct
                switch_hyprland(device, *restore);
                switch_hyprland(physical, *restore);
            }
            LayoutTarget::Sway {
                device,
                physical,
                restore,
            } => {
                switch_sway(device, *restore);
                switch_sway(physical, *restore);
            }
            LayoutTarget::Kde { restore } => switch_kde(*restore),
            LayoutTarget::Gnome { restore } => switch_gnome(*restore),
        }
    }
}

fn output_of(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

fn hyprland_target() -> Option<LayoutTarget> {
    let text = output_of("hyprctl", &["devices", "-j"])?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let keyboards = json.get("keyboards")?.as_array()?;
    let is_virtual = |keyboard: &&Value| {
        keyboard
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.to_ascii_lowercase().contains("molvai"))
    };
    // Hyprland marks the last device that sent a key as main, and that may be the virtual one.
    // The layout to restore belongs to a real keyboard
    let main = keyboards
        .iter()
        .filter(|keyboard| !is_virtual(keyboard))
        .find(|keyboard| keyboard.get("main").and_then(Value::as_bool) == Some(true))
        .or_else(|| keyboards.iter().find(|keyboard| !is_virtual(keyboard)))?;
    let restore = u32::try_from(main.get("active_layout_index")?.as_u64()?).ok()?;
    let physical = main.get("name")?.as_str()?.to_string();
    let device = keyboards
        .iter()
        .find(|keyboard| is_virtual(keyboard))
        .and_then(|keyboard| keyboard.get("name")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "molvai-virtual-keyboard".to_string());
    Some(LayoutTarget::Hyprland {
        device,
        physical,
        restore,
    })
}

fn switch_hyprland(device: &str, index: u32) {
    let _ = Command::new("hyprctl")
        .args(["switchxkblayout", device, &index.to_string()])
        .status();
}

fn sway_target() -> Option<LayoutTarget> {
    let text = output_of("swaymsg", &["-t", "get_inputs", "-r"])?;
    let inputs: Value = serde_json::from_str(&text).ok()?;
    let inputs = inputs.as_array()?;
    let is_virtual = |input: &&Value| {
        input
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.to_ascii_lowercase().contains("molvai"))
    };
    let physical = inputs.iter().find(|input| {
        !is_virtual(input) && input.get("type").and_then(Value::as_str) == Some("keyboard")
    })?;
    let restore = u32::try_from(physical.get("xkb_active_layout_index")?.as_u64()?).ok()?;
    let physical = physical
        .get("identifier")
        .or_else(|| physical.get("name"))
        .and_then(Value::as_str)?
        .to_string();
    let device = inputs
        .iter()
        .find(|input| is_virtual(input))
        .and_then(|input| input.get("identifier")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "molvai-virtual-keyboard".to_string());
    Some(LayoutTarget::Sway {
        device,
        physical,
        restore,
    })
}

fn switch_sway(device: &str, index: u32) {
    let _ = Command::new("swaymsg")
        .args(["input", device, "xkb_switch_layout", &index.to_string()])
        .status();
}

fn kde_output(method: &str) -> Option<String> {
    for command in ["qdbus6", "qdbus"] {
        if let Some(text) = output_of(command, &["org.kde.keyboard", "/Layouts", method]) {
            return Some(text.trim().to_string());
        }
    }
    None
}

fn kde_target() -> Option<LayoutTarget> {
    let restore = kde_output("org.kde.KeyboardLayouts.getLayout")?
        .parse()
        .ok()?;
    Some(LayoutTarget::Kde { restore })
}

fn switch_kde(index: u32) {
    for command in ["qdbus6", "qdbus"] {
        if Command::new(command)
            .args([
                "org.kde.keyboard",
                "/Layouts",
                "org.kde.KeyboardLayouts.setLayout",
                &index.to_string(),
            ])
            .status()
            .is_ok_and(|status| status.success())
        {
            break;
        }
    }
}

/// Entries of a GNOME input-source list, from the textual form of `gsettings`
fn xkb_entries(raw: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("('xkb'") {
        let tail = &rest[start..];
        let Some(end) = tail.find(')') else { break };
        entries.push(tail[..=end].to_string());
        rest = &tail[end + 1..];
    }
    entries
}

fn gnome_target() -> Option<LayoutTarget> {
    let sources = output_of(
        "gsettings",
        &["get", "org.gnome.desktop.input-sources", "sources"],
    )?;
    let mru = output_of(
        "gsettings",
        &["get", "org.gnome.desktop.input-sources", "mru-sources"],
    )?;
    let entries = xkb_entries(&sources);
    let active = xkb_entries(&mru).into_iter().next();
    let restore = active
        .and_then(|active| entries.iter().position(|entry| *entry == active))
        .unwrap_or(0);
    Some(LayoutTarget::Gnome { restore })
}

fn switch_gnome(index: usize) {
    let script = format!(
        "imports.ui.status.keyboard.getInputSourceManager().inputSources[{index}].activate()"
    );
    let _ = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell",
            "--method",
            "org.gnome.Shell.Eval",
            &script,
        ])
        .status();
}

const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_C: u16 = 46;
const KEY_V: u16 = 47;

/// Letters of the US layout: character → key code
const LETTERS: [(char, u16); 26] = [
    ('a', 30),
    ('b', 48),
    ('c', 46),
    ('d', 32),
    ('e', 18),
    ('f', 33),
    ('g', 34),
    ('h', 35),
    ('i', 23),
    ('j', 36),
    ('k', 37),
    ('l', 38),
    ('m', 50),
    ('n', 49),
    ('o', 24),
    ('p', 25),
    ('q', 16),
    ('r', 19),
    ('s', 31),
    ('t', 20),
    ('u', 22),
    ('v', 47),
    ('w', 17),
    ('x', 45),
    ('y', 21),
    ('z', 44),
];

/// Punctuation of the US layout: character → (code, whether Shift is needed)
const PUNCTUATION: [(char, u16, bool); 42] = [
    (' ', 57, false),
    ('\n', 28, false),
    ('\t', 15, false),
    ('1', 2, false),
    ('2', 3, false),
    ('3', 4, false),
    ('4', 5, false),
    ('5', 6, false),
    ('6', 7, false),
    ('7', 8, false),
    ('8', 9, false),
    ('9', 10, false),
    ('0', 11, false),
    ('!', 2, true),
    ('@', 3, true),
    ('#', 4, true),
    ('$', 5, true),
    ('%', 6, true),
    ('^', 7, true),
    ('&', 8, true),
    ('*', 9, true),
    ('(', 10, true),
    (')', 11, true),
    ('-', 12, false),
    ('_', 12, true),
    ('=', 13, false),
    ('+', 13, true),
    ('[', 26, false),
    ('{', 26, true),
    (']', 27, false),
    ('}', 27, true),
    (';', 39, false),
    (':', 39, true),
    ('\'', 40, false),
    ('"', 40, true),
    ('`', 41, false),
    ('~', 41, true),
    ('\\', 43, false),
    ('|', 43, true),
    (',', 51, false),
    ('<', 51, true),
    ('.', 52, false),
];

/// The rest of the punctuation, which did not fit in the table above without duplicates
const PUNCTUATION_TAIL: [(char, u16, bool); 3] =
    [('>', 52, true), ('/', 53, false), ('?', 53, true)];

/// Key code and the Shift flag for a character of the US layout
pub fn key_for(ch: char) -> Option<(u16, bool)> {
    if ch.is_ascii_alphabetic() {
        let lower = ch.to_ascii_lowercase();
        let code = LETTERS.iter().find(|(c, _)| *c == lower).map(|(_, k)| *k)?;
        return Some((code, ch.is_ascii_uppercase()));
    }
    PUNCTUATION
        .iter()
        .chain(PUNCTUATION_TAIL.iter())
        .find(|(c, _, _)| *c == ch)
        .map(|(_, code, shift)| (*code, *shift))
}

/// Whether the whole text can be typed with codes of the US layout
pub fn is_typable(text: &str) -> bool {
    text.chars().all(|ch| key_for(ch).is_some())
}

/// All codes that the device must declare at creation
fn declared_keys() -> AttributeSet<KeyCode> {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode(KEY_LEFTCTRL));
    keys.insert(KeyCode(KEY_LEFTSHIFT));
    for (_, code) in LETTERS {
        keys.insert(KeyCode(code));
    }
    for (_, code, _) in PUNCTUATION {
        keys.insert(KeyCode(code));
    }
    for (_, code, _) in PUNCTUATION_TAIL {
        keys.insert(KeyCode(code));
    }
    keys
}

#[derive(Debug)]
pub struct UinputInjector {
    device: Option<VirtualDevice>,
    clipboard: ClipboardGuard<SystemClipboard>,
    shortcut: PasteShortcut,
    type_delay: Duration,
}

impl UinputInjector {
    pub fn new(
        restore_clipboard: bool,
        restore_delay_ms: u32,
        shortcut: PasteShortcut,
        type_delay_ms: u32,
    ) -> Self {
        Self {
            device: None,
            clipboard: ClipboardGuard::new(
                SystemClipboard::new(),
                restore_clipboard,
                Duration::from_millis(u64::from(restore_delay_ms)),
            ),
            shortcut,
            type_delay: Duration::from_millis(u64::from(type_delay_ms)),
        }
    }

    /// Available means `/dev/uinput` really opens for writing
    ///
    /// A check that the file exists is not enough: without a udev rule it exists but belongs to
    /// root
    pub fn is_available() -> bool {
        Path::new(UINPUT_PATH).exists()
            && std::fs::OpenOptions::new()
                .write(true)
                .open(UINPUT_PATH)
                .is_ok()
    }

    /// Change the paste key combination: terminals need Ctrl+Shift+V
    pub fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        self.shortcut = shortcut;
    }

    fn device(&mut self) -> Result<&mut VirtualDevice, InjectError> {
        if self.device.is_none() {
            let device = VirtualDevice::builder()
                .and_then(|b| {
                    b.name("MolvAI virtual keyboard")
                        .with_keys(&declared_keys())
                })
                .and_then(evdev::uinput::VirtualDeviceBuilder::build)
                .map_err(|e| InjectError::Unavailable(format!("{UINPUT_PATH}: {e}")))?;
            // The compositor needs a moment to notice the new input device
            std::thread::sleep(Duration::from_millis(200));
            self.device = Some(device);
        }
        self.device
            .as_mut()
            .ok_or_else(|| InjectError::Unavailable(UINPUT_PATH.into()))
    }

    fn emit(device: &mut VirtualDevice, events: &[InputEvent]) -> Result<(), InjectError> {
        device
            .emit(events)
            .map_err(|e| InjectError::Failed(format!("uinput: {e}")))
    }

    fn key(code: u16, value: i32) -> InputEvent {
        InputEvent::new(EventType::KEY.0, code, value)
    }

    fn type_text(&mut self, text: &str) -> Result<(), InjectError> {
        if !is_typable(text) {
            return Err(InjectError::UnsupportedCharacters);
        }
        let delay = self.type_delay;
        let device = self.device()?;
        for ch in text.chars() {
            let Some((code, shift)) = key_for(ch) else {
                return Err(InjectError::UnsupportedCharacters);
            };
            if shift {
                Self::emit(device, &[Self::key(KEY_LEFTSHIFT, 1)])?;
            }
            Self::emit(device, &[Self::key(code, 1)])?;
            Self::emit(device, &[Self::key(code, 0)])?;
            if shift {
                Self::emit(device, &[Self::key(KEY_LEFTSHIFT, 0)])?;
            }
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
        }
        Ok(())
    }

    fn send_combo(&mut self, key: u16, shift: bool) -> Result<(), InjectError> {
        // Read the physical layout before the device exists: Hyprland may make the new device
        // main as soon as it appears
        let guard = LayoutGuard::prepare();
        let device = self.device()?;
        if let Some(guard) = &guard {
            guard.before();
        }
        let result = (|| {
            Self::emit(device, &[Self::key(KEY_LEFTCTRL, 1)])?;
            if shift {
                Self::emit(device, &[Self::key(KEY_LEFTSHIFT, 1)])?;
            }
            Self::emit(device, &[Self::key(key, 1)])?;
            Self::emit(device, &[Self::key(key, 0)])?;
            if shift {
                Self::emit(device, &[Self::key(KEY_LEFTSHIFT, 0)])?;
            }
            Self::emit(device, &[Self::key(KEY_LEFTCTRL, 0)])?;
            Ok::<(), InjectError>(())
        })();
        if let Some(guard) = &guard {
            guard.after();
        }
        result
    }

    fn send_paste(&mut self) -> Result<(), InjectError> {
        let shift = self.shortcut == PasteShortcut::CtrlShiftV;
        self.send_combo(KEY_V, shift)
    }
}

impl TextInjector for UinputInjector {
    fn id(&self) -> &'static str {
        "uinput"
    }

    fn available(&self) -> bool {
        self.device.is_some() || Self::is_available()
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        match mode {
            OutputMode::Type => {
                self.type_text(text)?;
                Ok(InjectReport {
                    method: "uinput-type".into(),
                    attempts: Vec::new(),
                })
            }
            OutputMode::Paste => {
                self.clipboard.stage(text)?;
                match self.send_paste() {
                    Ok(()) => {
                        self.clipboard.restore()?;
                        Ok(InjectReport {
                            method: "uinput-paste".into(),
                            attempts: Vec::new(),
                        })
                    }
                    Err(error) => {
                        self.clipboard.keep();
                        Err(error)
                    }
                }
            }
            _ => Err(InjectError::Unsupported),
        }
    }

    /// Command mode: copy the selection with Ctrl+C and take it from the clipboard
    ///
    /// This path needs neither a compositor command nor a third-party utility, so it is the
    /// fallback for a session where `hyprctl` refuses the shortcut. The layout guard is the same
    /// as for a paste: the device may become main, and its group would reach the client
    fn copy_selection(&mut self) -> Result<String, InjectError> {
        copy_selection_with(|| self.send_combo(KEY_C, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_sources_are_read_from_the_gsettings_form() {
        assert_eq!(
            xkb_entries("@as [('xkb', 'us'), ('xkb', 'ru')]"),
            vec!["('xkb', 'us')".to_string(), "('xkb', 'ru')".to_string()]
        );
        assert_eq!(
            xkb_entries("[('xkb', 'ru'), ('xkb', 'de')]"),
            vec!["('xkb', 'ru')".to_string(), "('xkb', 'de')".to_string()]
        );
    }

    #[test]
    fn ascii_letters_map_to_us_layout_codes() {
        assert_eq!(key_for('a'), Some((30, false)));
        assert_eq!(key_for('A'), Some((30, true)));
        assert_eq!(key_for('v'), Some((47, false)));
        assert_eq!(key_for('z'), Some((44, false)));
    }

    #[test]
    fn punctuation_carries_the_shift_flag() {
        assert_eq!(key_for('1'), Some((2, false)));
        assert_eq!(key_for('!'), Some((2, true)));
        assert_eq!(key_for('?'), Some((53, true)));
        assert_eq!(key_for('/'), Some((53, false)));
        assert_eq!(key_for(' '), Some((57, false)));
        assert_eq!(key_for('\n'), Some((28, false)));
    }

    /// Each pair of the US layout shares one code: the lower character without Shift, the upper
    /// one with it. A swapped flag would type `'` instead of `"` — and no other test would notice
    #[test]
    fn every_shift_pair_shares_a_code_and_differs_only_by_shift() {
        let pairs = [
            ('1', '!'),
            ('2', '@'),
            ('3', '#'),
            ('4', '$'),
            ('5', '%'),
            ('6', '^'),
            ('7', '&'),
            ('8', '*'),
            ('9', '('),
            ('0', ')'),
            ('-', '_'),
            ('=', '+'),
            ('[', '{'),
            (']', '}'),
            (';', ':'),
            ('\'', '"'),
            ('`', '~'),
            ('\\', '|'),
            (',', '<'),
            ('.', '>'),
            ('/', '?'),
        ];
        for (lower, upper) in pairs {
            let (lower_code, lower_shift) = key_for(lower).unwrap_or_else(|| panic!("{lower}"));
            let (upper_code, upper_shift) = key_for(upper).unwrap_or_else(|| panic!("{upper}"));
            assert_eq!(lower_code, upper_code, "{lower} and {upper} share one key");
            assert!(!lower_shift, "{lower} is typed without Shift");
            assert!(upper_shift, "{upper} is typed with Shift");
        }
    }

    #[test]
    fn cyrillic_and_emoji_have_no_key_code() {
        assert_eq!(key_for('я'), None);
        assert_eq!(key_for('—'), None);
        assert_eq!(key_for('🙂'), None);
    }

    #[test]
    fn typability_is_decided_for_the_whole_text() {
        assert!(is_typable("Hello, world! (test #1)"));
        assert!(!is_typable("привет"));
        assert!(!is_typable("mixed привет"));
        assert!(is_typable(""));
    }

    #[test]
    fn every_declared_key_has_a_code_and_the_set_is_not_empty() {
        let keys = declared_keys();
        assert!(keys.iter().count() > 40, "the device must declare all keys");
        assert!(keys.contains(KeyCode(KEY_V)));
        assert!(keys.contains(KeyCode(KEY_C)));
        assert!(keys.contains(KeyCode(KEY_LEFTCTRL)));
    }

    #[test]
    fn the_copy_key_is_the_physical_c_of_the_us_layout() {
        // The device sends key codes and the compositor applies the layout: 46 is C in any
        // layout, so voice editing copies with the Russian layout active too
        assert_eq!(KEY_C, 46);
        assert_eq!(key_for('c'), Some((KEY_C, false)));
        assert_ne!(KEY_C, KEY_V);
    }

    #[test]
    fn non_ascii_text_is_refused_before_any_device_is_opened() {
        let mut injector = UinputInjector::new(true, 0, PasteShortcut::CtrlV, 0);
        // The device is not created: the refusal comes before any access to /dev/uinput
        assert_eq!(
            injector.inject("привет", OutputMode::Type),
            Err(InjectError::UnsupportedCharacters)
        );
        assert!(injector.device.is_none());
    }

    #[test]
    fn clipboard_only_mode_is_not_this_injectors_job() {
        let mut injector = UinputInjector::new(true, 0, PasteShortcut::CtrlV, 0);
        assert_eq!(
            injector.inject("text", OutputMode::Clipboard),
            Err(InjectError::Unsupported)
        );
    }
}
