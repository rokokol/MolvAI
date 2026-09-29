// SPDX-License-Identifier: MIT
//! Вставка через enigo: X11, Windows, macOS и Wayland там, где композитор даёт виртуальную
//! клавиатуру.
//!
//! Подключение к серверу ввода делается лениво: на голом Wayland без нужного протокола enigo
//! не поднимется, и это не должно ронять демон при старте.

use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use crate::infra::inject::clipboard::{ClipboardGuard, SystemClipboard};
use crate::infra::inject::wayland_tools::PasteShortcut;

#[derive(Debug)]
pub struct EnigoInjector {
    enigo: Option<Enigo>,
    clipboard: ClipboardGuard<SystemClipboard>,
    shortcut: PasteShortcut,
}

impl EnigoInjector {
    pub fn new(restore_clipboard: bool, restore_delay_ms: u32, shortcut: PasteShortcut) -> Self {
        Self {
            enigo: None,
            clipboard: ClipboardGuard::new(
                SystemClipboard::new(),
                restore_clipboard,
                Duration::from_millis(u64::from(restore_delay_ms)),
            ),
            shortcut,
        }
    }

    /// Можно ли вообще подключиться к серверу ввода.
    pub fn is_available() -> bool {
        Enigo::new(&Settings::default()).is_ok()
    }

    /// Сменить сочетание вставки: терминалам нужен Ctrl+Shift+V.
    pub fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        self.shortcut = shortcut;
    }

    fn enigo(&mut self) -> Result<&mut Enigo, InjectError> {
        if self.enigo.is_none() {
            self.enigo = Some(
                Enigo::new(&Settings::default())
                    .map_err(|e| InjectError::Unavailable(format!("enigo: {e}")))?,
            );
        }
        self.enigo
            .as_mut()
            .ok_or_else(|| InjectError::Unavailable("enigo не поднялся".into()))
    }

    /// Клавиша V сочетания вставки.
    ///
    /// Windows и macOS ищут `Key::Unicode('v')` в текущей раскладке, и в ЙЦУКЕН такой
    /// клавиши нет: Ctrl+V не доходит до приложения. Физический код от раскладки не зависит.
    fn paste_key() -> Key {
        #[cfg(target_os = "windows")]
        {
            Key::Other(0x56) // VK_V
        }
        #[cfg(target_os = "macos")]
        {
            Key::Other(9) // kVK_ANSI_V
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Key::Unicode('v')
        }
    }

    /// Клавиша-модификатор вставки: на macOS это Cmd, на остальных — Ctrl.
    fn paste_modifier() -> Key {
        #[cfg(target_os = "macos")]
        {
            Key::Meta
        }
        #[cfg(not(target_os = "macos"))]
        {
            Key::Control
        }
    }

    fn send_paste(&mut self) -> Result<(), InjectError> {
        let shift = self.shortcut == PasteShortcut::CtrlShiftV;
        let modifier = Self::paste_modifier();
        let enigo = self.enigo()?;
        let map = |e: Result<(), enigo::InputError>| {
            e.map_err(|error| InjectError::Failed(format!("enigo: {error}")))
        };
        map(enigo.key(modifier, Direction::Press))?;
        if shift {
            map(enigo.key(Key::Shift, Direction::Press))?;
        }
        map(enigo.key(Self::paste_key(), Direction::Click))?;
        if shift {
            map(enigo.key(Key::Shift, Direction::Release))?;
        }
        map(enigo.key(modifier, Direction::Release))?;
        Ok(())
    }
}

impl TextInjector for EnigoInjector {
    fn id(&self) -> &'static str {
        "enigo"
    }

    fn available(&self) -> bool {
        self.enigo.is_some() || Self::is_available()
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        match mode {
            OutputMode::Type => {
                let enigo = self.enigo()?;
                enigo
                    .text(text)
                    .map_err(|e| InjectError::Failed(format!("enigo: {e}")))?;
                Ok(InjectReport {
                    method: "enigo-type".into(),
                    attempts: Vec::new(),
                })
            }
            OutputMode::Paste => {
                self.clipboard.stage(text)?;
                match self.send_paste() {
                    Ok(()) => {
                        self.clipboard.restore()?;
                        Ok(InjectReport {
                            method: "enigo-paste".into(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_only_mode_is_not_this_injectors_job() {
        let mut injector = EnigoInjector::new(true, 0, PasteShortcut::CtrlV);
        assert_eq!(
            injector.inject("текст", OutputMode::Clipboard),
            Err(InjectError::Unsupported)
        );
    }

    #[test]
    fn paste_modifier_matches_the_platform() {
        let expected = if cfg!(target_os = "macos") {
            Key::Meta
        } else {
            Key::Control
        };
        assert_eq!(EnigoInjector::paste_modifier(), expected);
    }

    #[test]
    fn paste_key_is_the_physical_v_key_where_the_layout_would_translate_it() {
        // Символ 'v' Windows и macOS ищут в текущей раскладке, а в ЙЦУКЕН его нет.
        let expected = if cfg!(target_os = "windows") {
            Key::Other(0x56) // VK_V
        } else if cfg!(target_os = "macos") {
            Key::Other(9) // kVK_ANSI_V
        } else {
            Key::Unicode('v')
        };
        assert_eq!(EnigoInjector::paste_key(), expected);
    }

    #[test]
    fn id_is_stable_for_the_journal() {
        assert_eq!(
            EnigoInjector::new(true, 0, PasteShortcut::CtrlV).id(),
            "enigo"
        );
    }
}
