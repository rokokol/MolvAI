// SPDX-License-Identifier: MIT
//! Parsing of combinations such as `Ctrl+Shift+Space`, `F9`, `RightCtrl`, `Pause`
//!
//! A combination becomes physical kernel key codes, not characters: `evdev` sees the codes,
//! and they depend on neither the layout nor the input language. So `Ctrl+Shift+Space` also
//! works in the Russian layout

use std::collections::BTreeSet;

use crate::domain::hotkeys::HotkeyError;

/// Modifier, with no difference between left and right
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

impl Modifier {
    /// Kernel key codes that give this modifier
    pub fn codes(self) -> [u16; 2] {
        match self {
            Modifier::Ctrl => [29, 97],
            Modifier::Shift => [42, 54],
            Modifier::Alt => [56, 100],
            Modifier::Super => [125, 126],
        }
    }

    /// Modifier that this key gives, if the key is a modifier
    pub fn from_code(code: u16) -> Option<Modifier> {
        [
            Modifier::Ctrl,
            Modifier::Shift,
            Modifier::Alt,
            Modifier::Super,
        ]
        .into_iter()
        .find(|m| m.codes().contains(&code))
    }

    fn parse(name: &str) -> Option<Modifier> {
        match name {
            "ctrl" | "control" => Some(Modifier::Ctrl),
            "shift" => Some(Modifier::Shift),
            "alt" | "option" => Some(Modifier::Alt),
            "super" | "meta" | "win" | "cmd" | "command" => Some(Modifier::Super),
            _ => None,
        }
    }
}

/// Named keys that do not derive from a letter or a digit: the name in the settings, the kernel
/// key code, and the xkb keysym that the compositors take in their own binds
const NAMED_KEYS: [(&str, u16, &str); 40] = [
    ("escape", 1, "Escape"),
    ("esc", 1, "Escape"),
    ("backspace", 14, "BackSpace"),
    ("tab", 15, "Tab"),
    ("enter", 28, "Return"),
    ("return", 28, "Return"),
    ("space", 57, "space"),
    ("capslock", 58, "Caps_Lock"),
    ("f1", 59, "F1"),
    ("f2", 60, "F2"),
    ("f3", 61, "F3"),
    ("f4", 62, "F4"),
    ("f5", 63, "F5"),
    ("f6", 64, "F6"),
    ("f7", 65, "F7"),
    ("f8", 66, "F8"),
    ("f9", 67, "F9"),
    ("f10", 68, "F10"),
    ("f11", 87, "F11"),
    ("f12", 88, "F12"),
    ("numlock", 69, "Num_Lock"),
    ("scrolllock", 70, "Scroll_Lock"),
    ("sysrq", 99, "Print"),
    ("printscreen", 99, "Print"),
    ("home", 102, "Home"),
    ("up", 103, "Up"),
    ("pageup", 104, "Page_Up"),
    ("left", 105, "Left"),
    ("right", 106, "Right"),
    ("end", 107, "End"),
    ("down", 108, "Down"),
    ("pagedown", 109, "Page_Down"),
    ("insert", 110, "Insert"),
    ("delete", 111, "Delete"),
    ("pause", 119, "Pause"),
    ("menu", 127, "Menu"),
    ("minus", 12, "minus"),
    ("equal", 13, "equal"),
    ("grave", 41, "grave"),
    ("backslash", 43, "backslash"),
];

/// Modifier keys that can be a hotkey on their own, with the kernel code and the keysym
const SIDED_MODIFIERS: [(&str, u16, &str); 8] = [
    ("leftctrl", 29, "Control_L"),
    ("rightctrl", 97, "Control_R"),
    ("leftshift", 42, "Shift_L"),
    ("rightshift", 54, "Shift_R"),
    ("leftalt", 56, "Alt_L"),
    ("rightalt", 100, "Alt_R"),
    ("leftsuper", 125, "Super_L"),
    ("rightsuper", 126, "Super_R"),
];

/// Letters of the US layout: the codes of the same keys as in `infra::inject::uinput`
const LETTER_CODES: [(char, u16); 26] = [
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

fn named(name: &str) -> Option<&'static (&'static str, u16, &'static str)> {
    NAMED_KEYS
        .iter()
        .chain(SIDED_MODIFIERS.iter())
        .find(|(n, _, _)| *n == name)
}

fn key_code(name: &str) -> Option<u16> {
    if let Some((_, code, _)) = named(name) {
        return Some(*code);
    }
    let mut chars = name.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if ch.is_ascii_digit() {
        // 1..9 are codes 2..10, 0 is code 11
        let digit = ch.to_digit(10)? as u16;
        return Some(if digit == 0 { 11 } else { digit + 1 });
    }
    LETTER_CODES
        .iter()
        .find(|(c, _)| *c == ch.to_ascii_lowercase())
        .map(|(_, code)| *code)
}

/// Parsed combination
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub modifiers: BTreeSet<Modifier>,
    /// Code of the main key
    pub key: u16,
    /// Source string, for error messages and for `doctor`
    pub source: String,
}

impl HotkeySpec {
    /// Parse `Ctrl+Shift+Space`. Case and spaces do not matter, and the separator is `+` or `-`
    pub fn parse(spec: &str) -> Result<HotkeySpec, HotkeyError> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return Err(HotkeyError::BadSpec("empty combination".into()));
        }
        let parts: Vec<String> = trimmed
            .split(['+', '-'])
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        // An empty list and the parsing of the last part are the same check: `split_last`
        // already answers the question "is there anything to parse"
        let Some((last, head)) = parts.split_last() else {
            return Err(HotkeyError::BadSpec(spec.to_string()));
        };
        let mut modifiers = BTreeSet::new();
        for part in head {
            let modifier = Modifier::parse(part)
                .ok_or_else(|| HotkeyError::BadSpec(format!("{spec}: unknown \"{part}\"")))?;
            modifiers.insert(modifier);
        }
        // The last part can be an ordinary key or a whole modifier:
        // `RightCtrl` as push-to-talk is exactly this case
        let key = key_code(last)
            .or_else(|| Modifier::parse(last).map(|m| m.codes()[0]))
            .ok_or_else(|| HotkeyError::BadSpec(format!("{spec}: unknown key \"{last}\"")))?;
        Ok(HotkeySpec {
            modifiers,
            key,
            source: trimmed.to_string(),
        })
    }

    /// Whether the combination fired: its key is pressed and exactly its modifiers are held
    pub fn matches(&self, code: u16, active: &BTreeSet<Modifier>) -> bool {
        if code != self.key {
            return false;
        }
        // A modifier that is itself the key of the combination need not be in the set:
        // `RightCtrl` is pressed, and it also enters the active modifiers
        if let Some(own) = Modifier::from_code(self.key) {
            return active.is_superset(&self.modifiers) && active.contains(&own);
        }
        self.modifiers == *active
    }

    /// The xkb keysym of the main key, the name a compositor takes in its binds
    ///
    /// It comes from the key code through the same tables that the parsing reads, so every
    /// spelling of a key (`esc`, `escape`) gives one keysym
    pub fn keysym(&self) -> String {
        if let Some((_, _, keysym)) = NAMED_KEYS
            .iter()
            .chain(SIDED_MODIFIERS.iter())
            .find(|(_, code, _)| *code == self.key)
        {
            return (*keysym).to_string();
        }
        if let Some((letter, _)) = LETTER_CODES.iter().find(|(_, code)| *code == self.key) {
            return letter.to_ascii_uppercase().to_string();
        }
        // Digits: codes 2..10 are 1..9, code 11 is 0
        match self.key {
            11 => "0".to_string(),
            code => (code - 1).to_string(),
        }
    }
}

impl Modifier {
    /// The modifier name as the compositors spell it, before their own case rules
    pub fn label(self) -> &'static str {
        match self {
            Modifier::Ctrl => "Ctrl",
            Modifier::Shift => "Shift",
            Modifier::Alt => "Alt",
            Modifier::Super => "Super",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(list: &[Modifier]) -> BTreeSet<Modifier> {
        list.iter().copied().collect()
    }

    #[test]
    fn a_combination_becomes_modifiers_and_a_key_code() {
        let spec = HotkeySpec::parse("Ctrl+Shift+Space").unwrap();
        assert_eq!(spec.modifiers, mods(&[Modifier::Ctrl, Modifier::Shift]));
        assert_eq!(spec.key, 57);
    }

    #[test]
    fn case_and_spaces_do_not_matter() {
        assert_eq!(
            HotkeySpec::parse("  ctrl + SHIFT + space ").unwrap().key,
            HotkeySpec::parse("Ctrl+Shift+Space").unwrap().key
        );
        assert_eq!(
            HotkeySpec::parse("ctrl-alt-s").unwrap().modifiers,
            mods(&[Modifier::Ctrl, Modifier::Alt])
        );
    }

    #[test]
    fn function_keys_and_pause_are_known() {
        assert_eq!(HotkeySpec::parse("F9").unwrap().key, 67);
        assert_eq!(HotkeySpec::parse("F12").unwrap().key, 88);
        assert_eq!(HotkeySpec::parse("Pause").unwrap().key, 119);
        assert_eq!(HotkeySpec::parse("Escape").unwrap().key, 1);
    }

    #[test]
    fn a_lone_modifier_is_a_valid_push_to_talk_key() {
        let spec = HotkeySpec::parse("RightCtrl").unwrap();
        assert!(spec.modifiers.is_empty());
        assert_eq!(spec.key, 97);
        assert!(spec.matches(97, &mods(&[Modifier::Ctrl])));
        assert!(
            !spec.matches(29, &mods(&[Modifier::Ctrl])),
            "left Ctrl is not right Ctrl"
        );
    }

    #[test]
    fn letters_and_digits_map_to_us_layout_codes() {
        assert_eq!(HotkeySpec::parse("Ctrl+Shift+Alt+S").unwrap().key, 31);
        assert_eq!(HotkeySpec::parse("Ctrl+1").unwrap().key, 2);
        assert_eq!(HotkeySpec::parse("Ctrl+0").unwrap().key, 11);
    }

    #[test]
    fn an_unknown_name_is_reported_with_the_whole_spec() {
        let error = HotkeySpec::parse("Ctrl+Телепатия").unwrap_err();
        assert!(matches!(error, HotkeyError::BadSpec(_)), "{error}");
        assert!(error.to_string().contains("Ctrl+Телепатия"), "{error}");
        assert!(HotkeySpec::parse("").is_err());
        assert!(HotkeySpec::parse("Хтоних+Space").is_err());
    }

    #[test]
    fn an_extra_modifier_does_not_trigger_the_combination() {
        let spec = HotkeySpec::parse("Ctrl+Space").unwrap();
        assert!(spec.matches(57, &mods(&[Modifier::Ctrl])));
        assert!(!spec.matches(57, &mods(&[Modifier::Ctrl, Modifier::Shift])));
        assert!(!spec.matches(57, &mods(&[])));
    }

    #[test]
    fn defaults_from_the_config_all_parse() {
        for spec in [
            "RightCtrl",
            "Ctrl+Shift+Space",
            "Ctrl+Shift+Alt+Space",
            "Escape",
            "Ctrl+Shift+Alt+S",
        ] {
            HotkeySpec::parse(spec).unwrap_or_else(|e| panic!("{spec}: {e}"));
        }
    }

    #[test]
    fn modifier_keys_are_recognised_by_their_codes() {
        assert_eq!(Modifier::from_code(29), Some(Modifier::Ctrl));
        assert_eq!(Modifier::from_code(54), Some(Modifier::Shift));
        assert_eq!(Modifier::from_code(126), Some(Modifier::Super));
        assert_eq!(Modifier::from_code(57), None);
    }

    fn keysym(spec: &str) -> String {
        HotkeySpec::parse(spec).unwrap().keysym()
    }

    #[test]
    fn every_spelling_of_a_key_gives_its_xkb_keysym() {
        assert_eq!(keysym("F9"), "F9");
        assert_eq!(keysym("f12"), "F12");
        assert_eq!(keysym("Ctrl+Space"), "space");
        assert_eq!(keysym("esc"), keysym("Escape"));
        assert_eq!(keysym("Escape"), "Escape");
        assert_eq!(keysym("PageDown"), "Page_Down");
        assert_eq!(keysym("Pause"), "Pause");
        assert_eq!(keysym("RightCtrl"), "Control_R");
        assert_eq!(keysym("Ctrl+Alt+z"), "Z");
        assert_eq!(keysym("Super+1"), "1");
        assert_eq!(keysym("Super+0"), "0");
    }

    #[test]
    fn a_lone_modifier_as_the_key_is_its_left_keysym() {
        assert_eq!(keysym("Ctrl"), "Control_L");
    }

    #[test]
    fn every_named_key_has_a_keysym_and_one_code_gives_one_keysym() {
        for (name, code, sym) in NAMED_KEYS.iter().chain(SIDED_MODIFIERS.iter()) {
            assert!(!sym.is_empty(), "{name}");
            let same_code = NAMED_KEYS
                .iter()
                .chain(SIDED_MODIFIERS.iter())
                .filter(|(_, c, _)| c == code);
            for (other, _, other_sym) in same_code {
                assert_eq!(sym, other_sym, "{name} and {other} share code {code}");
            }
        }
    }
}
