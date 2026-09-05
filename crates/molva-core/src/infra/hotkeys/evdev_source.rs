// SPDX-License-Identifier: MIT
//! Горячие клавиши поверх `/dev/input/event*`.
//!
//! Это запасной бэкенд: он видит клавиши до композитора, поэтому работает и там, где глобальных
//! биндов нет, но требует прав на устройства ввода (обычно группа `input`). Модификаторы
//! отслеживаются по физическим кодам, а не по символам, поэтому раскладка ни на что не влияет.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use evdev::{Device, EventType, KeyCode};

use crate::app::hotkeys::spec::{HotkeySpec, Modifier};
use crate::domain::hotkeys::{HotkeyAction, HotkeyError, HotkeyEvent, HotkeySource, KeyState};

/// Значения поля `value` события клавиши в ядре.
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

    /// Есть ли хоть одна клавиатура, которую мы вправе читать.
    pub fn available() -> bool {
        !keyboards().is_empty()
    }

    /// Пути к клавиатурам, которые удалось открыть: для `molva doctor`.
    pub fn devices() -> Vec<String> {
        keyboards()
            .into_iter()
            .map(|(path, device)| {
                format!(
                    "{} — {}",
                    path.display(),
                    device.name().unwrap_or("без имени")
                )
            })
            .collect()
    }
}

/// Клавиатура — это устройство, которое умеет буквы: так отсеиваются мыши и кнопки питания.
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
                "нет доступных клавиатур в /dev/input: добавьте пользователя в группу input".into(),
            ));
        }
        // Модификаторы общие для всех устройств: Ctrl на встроенной клавиатуре и Space на
        // внешней — это одна комбинация с точки зрения пользователя.
        let modifiers = Arc::new(Mutex::new(BTreeSet::<Modifier>::new()));
        let specs = Arc::new(self.specs);
        let mut threads = Vec::new();
        for (path, device) in devices {
            let tx = tx.clone();
            let modifiers = modifiers.clone();
            let specs = specs.clone();
            threads.push(std::thread::spawn(move || {
                if let Err(error) = pump(device, &tx, &modifiers, &specs) {
                    tracing::warn!(device = %path.display(), %error, "чтение устройства прекращено");
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
                // Автоповтор клавиши не создаёт новых нажатий.
                _ => continue,
            };
            // Снимок модификаторов берётся до обновления: при нажатии самой клавиши-модификатора
            // комбинация `RightCtrl` должна видеть его уже нажатым, а `Ctrl+Space` — нет.
            let mut active = {
                let mut guard = modifiers
                    .lock()
                    .map_err(|_| HotkeyError::Backend("состояние модификаторов потеряно".into()))?;
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
        // Значения ядра зафиксированы в linux/input.h и меняться не могут.
        assert_eq!(VALUE_PRESS, 1);
        assert_eq!(VALUE_RELEASE, 0);
    }

    #[test]
    fn enumerating_devices_never_panics_even_without_permissions() {
        // В CI и в контейнере /dev/input пуст или закрыт: список просто пустой.
        let _ = EvdevHotkeys::devices();
        let _ = EvdevHotkeys::available();
    }
}
