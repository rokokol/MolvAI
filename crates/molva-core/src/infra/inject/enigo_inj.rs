// SPDX-License-Identifier: MIT
//! Paste through enigo: X11, Windows, macOS and Wayland where the compositor gives a virtual
//! keyboard
//!
//! The connection to the input server is lazy: on bare Wayland without the needed protocol
//! enigo does not start, and this must not crash the daemon at startup

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

    /// Whether a connection to the input server is possible at all
    pub fn is_available() -> bool {
        Enigo::new(&Settings::default()).is_ok()
    }

    /// Change the paste key combination: terminals need Ctrl+Shift+V
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
            .ok_or_else(|| InjectError::Unavailable("enigo did not start".into()))
    }

    /// The V key of the paste key combination
    ///
    /// Windows and macOS look for `Key::Unicode('v')` in the current layout, and the Russian
    /// JCUKEN layout has no such key: Ctrl+V does not reach the application. The physical code
    /// does not depend on the layout
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

    /// The modifier key of the paste: Cmd on macOS, Ctrl elsewhere
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
        // Windows and macOS look for the character 'v' in the current layout, and JCUKEN has none
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
