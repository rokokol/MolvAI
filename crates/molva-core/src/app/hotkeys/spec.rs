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

/// Named keys that do not derive from a letter or a digit
const NAMED_KEYS: [(&str, u16); 40] = [
    ("escape", 1),
    ("esc", 1),
    ("backspace", 14),
    ("tab", 15),
    ("enter", 28),
    ("return", 28),
    ("space", 57),
    ("capslock", 58),
    ("f1", 59),
    ("f2", 60),
    ("f3", 61),
    ("f4", 62),
    ("f5", 63),
    ("f6", 64),
    ("f7", 65),
    ("f8", 66),
    ("f9", 67),
    ("f10", 68),
    ("f11", 87),
    ("f12", 88),
    ("numlock", 69),
    ("scrolllock", 70),
    ("sysrq", 99),
    ("printscreen", 99),
    ("home", 102),
    ("up", 103),
    ("pageup", 104),
    ("left", 105),
    ("right", 106),
    ("end", 107),
    ("down", 108),
    ("pagedown", 109),
    ("insert", 110),
    ("delete", 111),
    ("pause", 119),
    ("menu", 127),
    ("minus", 12),
    ("equal", 13),
    ("grave", 41),
    ("backslash", 43),
];

/// Modifier keys that can be a hotkey on their own
const SIDED_MODIFIERS: [(&str, u16); 8] = [
    ("leftctrl", 29),
    ("rightctrl", 97),
    ("leftshift", 42),
    ("rightshift", 54),
    ("leftalt", 56),
    ("rightalt", 100),
    ("leftsuper", 125),
    ("rightsuper", 126),
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

fn key_code(name: &str) -> Option<u16> {
    if let Some((_, code)) = NAMED_KEYS.iter().find(|(n, _)| *n == name) {
        return Some(*code);
    }
    if let Some((_, code)) = SIDED_MODIFIERS.iter().find(|(n, _)| *n == name) {
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
}
