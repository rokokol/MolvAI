// SPDX-License-Identifier: MIT
//! Вставка через собственное виртуальное устройство `/dev/uinput`.
//!
//! Это единственный способ, который не зависит ни от композитора, ни от сторонних демонов, но
//! он требует прав на `/dev/uinput` (обычно правило udev на группу `input`). Раскладку
//! устройство не знает: ядру уходят коды клавиш, а раскладку применяет уже композитор, поэтому
//! набирать вслепую можно только ASCII по US-раскладке. Всё остальное идёт через буфер обмена.

use std::path::Path;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};

use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use crate::infra::inject::clipboard::{ClipboardGuard, SystemClipboard};
use crate::infra::inject::wayland_tools::PasteShortcut;

const UINPUT_PATH: &str = "/dev/uinput";

const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_V: u16 = 47;

/// Буквы US-раскладки: символ → код клавиши.
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

/// Знаки US-раскладки: символ → (код, нужен ли Shift).
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

/// Остаток знаков, не поместившийся в таблицу выше без дублей.
const PUNCTUATION_TAIL: [(char, u16, bool); 3] =
    [('>', 52, true), ('/', 53, false), ('?', 53, true)];

/// Код клавиши и признак Shift для символа US-раскладки.
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

/// Можно ли набрать весь текст кодами US-раскладки.
pub fn is_typable(text: &str) -> bool {
    text.chars().all(|ch| key_for(ch).is_some())
}

/// Все коды, которые устройство обязано объявить при создании.
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

    /// Доступность = `/dev/uinput` реально открывается на запись.
    ///
    /// Проверять существование файла мало: без правила udev он есть, но принадлежит root.
    pub fn is_available() -> bool {
        Path::new(UINPUT_PATH).exists()
            && std::fs::OpenOptions::new()
                .write(true)
                .open(UINPUT_PATH)
                .is_ok()
    }

    /// Сменить сочетание вставки: терминалам нужен Ctrl+Shift+V.
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
                .and_then(|b| b.build())
                .map_err(|e| InjectError::Unavailable(format!("{UINPUT_PATH}: {e}")))?;
            // Композитору нужно мгновение, чтобы заметить новое устройство ввода.
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

    fn send_paste(&mut self) -> Result<(), InjectError> {
        let shift = self.shortcut == PasteShortcut::CtrlShiftV;
        let device = self.device()?;
        Self::emit(device, &[Self::key(KEY_LEFTCTRL, 1)])?;
        if shift {
            Self::emit(device, &[Self::key(KEY_LEFTSHIFT, 1)])?;
        }
        Self::emit(device, &[Self::key(KEY_V, 1)])?;
        Self::emit(device, &[Self::key(KEY_V, 0)])?;
        if shift {
            Self::emit(device, &[Self::key(KEY_LEFTSHIFT, 0)])?;
        }
        Self::emit(device, &[Self::key(KEY_LEFTCTRL, 0)])?;
        Ok(())
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
                    Err(err) => {
                        self.clipboard.keep();
                        Err(err)
                    }
                }
            }
            _ => Err(InjectError::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Каждая пара US-раскладки делит один код: нижний символ без Shift, верхний — с ним.
    /// Перепутанный флаг напечатал бы `'` вместо `"` — и ни один другой тест этого не заметит.
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
            assert_eq!(
                lower_code, upper_code,
                "{lower} и {upper} делят одну клавишу"
            );
            assert!(!lower_shift, "{lower} набирается без Shift");
            assert!(upper_shift, "{upper} набирается с Shift");
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
        assert!(
            keys.iter().count() > 40,
            "устройство должно объявить все клавиши"
        );
        assert!(keys.contains(KeyCode(KEY_V)));
        assert!(keys.contains(KeyCode(KEY_LEFTCTRL)));
    }

    #[test]
    fn non_ascii_text_is_refused_before_any_device_is_opened() {
        let mut injector = UinputInjector::new(true, 0, PasteShortcut::CtrlV, 0);
        // Устройство не создаётся: отказ виден до обращения к /dev/uinput.
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
