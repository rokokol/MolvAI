// SPDX-License-Identifier: MIT
//! Разбор комбинаций вида `Ctrl+Shift+Space`, `F9`, `RightCtrl`, `Pause`.
//!
//! Комбинация превращается в физические коды клавиш ядра, а не в символы: `evdev` видит именно
//! коды, и они не зависят ни от раскладки, ни от языка ввода. Поэтому `Ctrl+Shift+Space`
//! работает и в русской раскладке.

use std::collections::BTreeSet;

use crate::domain::hotkeys::HotkeyError;

/// Модификатор без различения левого и правого.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

impl Modifier {
    /// Коды клавиш ядра, дающие этот модификатор.
    pub fn codes(self) -> [u16; 2] {
        match self {
            Modifier::Ctrl => [29, 97],
            Modifier::Shift => [42, 54],
            Modifier::Alt => [56, 100],
            Modifier::Super => [125, 126],
        }
    }

    /// Модификатор, который даёт эта клавиша, если она модификатор.
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

/// Именованные клавиши, которые не выводятся из буквы или цифры.
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

/// Клавиши-модификаторы, которые можно назначить хоткеем сами по себе.
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

/// Буквы US-раскладки: коды тех же клавиш, что и в `infra::inject::uinput`.
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
        // 1..9 — коды 2..10, 0 — код 11.
        let digit = ch.to_digit(10)? as u16;
        return Some(if digit == 0 { 11 } else { digit + 1 });
    }
    LETTER_CODES
        .iter()
        .find(|(c, _)| *c == ch.to_ascii_lowercase())
        .map(|(_, code)| *code)
}

/// Разобранная комбинация.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub modifiers: BTreeSet<Modifier>,
    /// Код основной клавиши.
    pub key: u16,
    /// Исходная строка — для сообщений об ошибках и для `doctor`.
    pub source: String,
}

impl HotkeySpec {
    /// Разобрать `Ctrl+Shift+Space`. Регистр и пробелы не важны, разделитель — `+` или `-`.
    pub fn parse(spec: &str) -> Result<HotkeySpec, HotkeyError> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return Err(HotkeyError::BadSpec("пустая комбинация".into()));
        }
        let parts: Vec<String> = trimmed
            .split(['+', '-'])
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        // Пустой список и разбор последней части — одна и та же проверка: `split_last` уже
        // отвечает на вопрос «есть ли что разбирать».
        let Some((last, head)) = parts.split_last() else {
            return Err(HotkeyError::BadSpec(spec.to_string()));
        };
        let mut modifiers = BTreeSet::new();
        for part in head {
            let modifier = Modifier::parse(part)
                .ok_or_else(|| HotkeyError::BadSpec(format!("{spec}: неизвестный «{part}»")))?;
            modifiers.insert(modifier);
        }
        // Последняя часть может быть и обычной клавишей, и модификатором целиком:
        // `RightCtrl` как push-to-talk — это ровно такой случай.
        let key = key_code(last)
            .or_else(|| Modifier::parse(last).map(|m| m.codes()[0]))
            .ok_or_else(|| HotkeyError::BadSpec(format!("{spec}: неизвестная клавиша «{last}»")))?;
        Ok(HotkeySpec {
            modifiers,
            key,
            source: trimmed.to_string(),
        })
    }

    /// Сработала ли комбинация: нажата её клавиша и ровно её модификаторы.
    pub fn matches(&self, code: u16, active: &BTreeSet<Modifier>) -> bool {
        if code != self.key {
            return false;
        }
        // Модификатор, который сам является клавишей комбинации, не обязан быть в наборе:
        // `RightCtrl` нажимается — и он же попадает в активные модификаторы.
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
            "левый Ctrl — не правый"
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
