// SPDX-License-Identifier: MIT
//! Hotkeys on top of `/dev/input/event*`
//!
//! This is a fallback backend: it sees the keys before the compositor, so it works where there
//! are no global binds, but it needs permission to read input devices (usually the `input`
//! group). Modifiers are tracked by physical codes, not by characters, so the keyboard layout
//! does not matter

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use evdev::{Device, EventType, KeyCode};

use crate::app::hotkeys::spec::{HotkeySpec, Modifier};
use crate::domain::hotkeys::{HotkeyAction, HotkeyError, HotkeyEvent, HotkeySource, KeyState};

/// Values of the `value` field of a key event in the kernel
const VALUE_RELEASE: i32 = 0;
const VALUE_PRESS: i32 = 1;

#[derive(Debug)]
pub struct EvdevHotkeys {
    specs: HashMap<HotkeyAction, HotkeySpec>,
}

impl EvdevHotkeys {
    pub fn new(specs: HashMap<HotkeyAction, HotkeySpec>) -> Self {
        Self { specs }
    }

    /// Whether there is at least one keyboard that we may read
    pub fn available() -> bool {
        !keyboards().is_empty()
    }

    /// Paths to the keyboards that could be opened: for `molva doctor`
    pub fn devices() -> Vec<String> {
        keyboards()
            .into_iter()
            .map(|(path, device)| {
                format!(
                    "{} — {}",
                    path.display(),
                    device.name().unwrap_or("unnamed")
                )
            })
            .collect()
    }
}

/// A keyboard is a device that has letters: this filters out mice and power buttons
fn keyboards() -> Vec<(PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z))
        })
        .collect()
}

impl HotkeySource for EvdevHotkeys {
    fn run(self: Box<Self>, tx: Sender<HotkeyEvent>) -> Result<(), HotkeyError> {
        let devices = keyboards();
        if devices.is_empty() {
            return Err(HotkeyError::Permission(
                "no keyboards available in /dev/input: add the user to the input group".into(),
            ));
        }
        // Modifiers are shared by all devices: Ctrl on the built-in keyboard and Space on an
        // external one are one combination from the point of view of the user
        let modifiers = Arc::new(Mutex::new(BTreeSet::<Modifier>::new()));
        let specs = Arc::new(self.specs);
        let mut threads = Vec::new();
        for (path, device) in devices {
            let tx = tx.clone();
            let modifiers = modifiers.clone();
            let specs = specs.clone();
            threads.push(std::thread::spawn(move || {
                if let Err(error) = pump(device, &tx, &modifiers, &specs) {
                    tracing::warn!(device = %path.display(), %error, "reading of the device stopped");
                }
            }));
        }
        for thread in threads {
            let _ = thread.join();
        }
        Ok(())
    }
}

fn pump(
    mut device: Device,
    tx: &Sender<HotkeyEvent>,
    modifiers: &Mutex<BTreeSet<Modifier>>,
    specs: &HashMap<HotkeyAction, HotkeySpec>,
) -> Result<(), HotkeyError> {
    loop {
        let events = device
            .fetch_events()
            .map_err(|e| HotkeyError::Backend(e.to_string()))?;
        for event in events {
            if event.event_type() != EventType::KEY {
                continue;
            }
            let code = event.code();
            let state = match event.value() {
                VALUE_PRESS => KeyState::Pressed,
                VALUE_RELEASE => KeyState::Released,
                // The key auto-repeat does not create new presses
                _ => continue,
            };
            // The snapshot of the modifiers is taken before the update: when the modifier key
            // itself is pressed, the combination `RightCtrl` must see it already pressed, and `Ctrl+Space` must not
            let mut active = {
                let mut guard = modifiers
                    .lock()
                    .map_err(|_| HotkeyError::Backend("modifier state lost".into()))?;
                if let Some(modifier) = Modifier::from_code(code) {
                    match state {
                        KeyState::Pressed => guard.insert(modifier),
                        KeyState::Released => guard.remove(&modifier),
                    };
                }
                guard.clone()
            };
            if let Some(modifier) = Modifier::from_code(code) {
                active.insert(modifier);
            }
            for (action, spec) in specs {
                if spec.matches(code, &active) {
                    let event = HotkeyEvent {
                        action: *action,
                        state,
                        at: std::time::Instant::now(),
                    };
                    if tx.send(event).is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_can_be_built_without_touching_any_device() {
        let mut specs = HashMap::new();
        specs.insert(
            HotkeyAction::PushToTalk,
            HotkeySpec::parse("RightCtrl").unwrap(),
        );
        let source = EvdevHotkeys::new(specs);
        assert_eq!(source.specs.len(), 1);
    }

    #[test]
    fn key_values_map_to_press_and_release() {
        // The kernel values are fixed in linux/input.h and cannot change
        assert_eq!(VALUE_PRESS, 1);
        assert_eq!(VALUE_RELEASE, 0);
    }

    #[test]
    fn enumerating_devices_never_panics_even_without_permissions() {
        // In CI and in a container /dev/input is empty or closed: the list is just empty
        let _ = EvdevHotkeys::devices();
        let _ = EvdevHotkeys::available();
    }
}
