// SPDX-License-Identifier: MIT
//! Горячие клавиши: разбор комбинаций и привязка их к действиям.

pub mod spec;

use std::collections::HashMap;

use crate::config::HotkeysConfig;
use crate::domain::hotkeys::{HotkeyAction, HotkeyError};

pub use spec::{HotkeySpec, Modifier};

/// Привязка действий к комбинациям из конфига.
///
/// Пустая строка в конфиге означает «действие не назначено», а не ошибку: пользователь вправе
/// отключить, например, отмену.
pub fn specs_from_config(
    config: &HotkeysConfig,
) -> Result<HashMap<HotkeyAction, HotkeySpec>, HotkeyError> {
    let pairs = [
        (HotkeyAction::PushToTalk, config.push_to_talk.as_str()),
        (HotkeyAction::Toggle, config.toggle.as_str()),
        (HotkeyAction::Command, config.command.as_str()),
        (HotkeyAction::Cancel, config.cancel.as_str()),
        (HotkeyAction::StyleNext, config.style_next.as_str()),
    ];
    let mut specs = HashMap::new();
    for (action, text) in pairs {
        if text.trim().is_empty() {
            continue;
        }
        specs.insert(action, HotkeySpec::parse(text)?);
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_binds_every_action() {
        let specs = specs_from_config(&HotkeysConfig::default()).unwrap();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[&HotkeyAction::PushToTalk].key, 97);
        assert_eq!(specs[&HotkeyAction::Cancel].key, 1);
    }

    #[test]
    fn an_empty_binding_is_omitted_not_rejected() {
        let config = HotkeysConfig {
            cancel: String::new(),
            style_next: "  ".into(),
            ..HotkeysConfig::default()
        };
        let specs = specs_from_config(&config).unwrap();
        assert!(!specs.contains_key(&HotkeyAction::Cancel));
        assert!(!specs.contains_key(&HotkeyAction::StyleNext));
        assert!(specs.contains_key(&HotkeyAction::PushToTalk));
    }

    #[test]
    fn a_broken_binding_names_the_action_that_broke() {
        let config = HotkeysConfig {
            toggle: "Ctrl+Хтоних".into(),
            ..HotkeysConfig::default()
        };
        let err = specs_from_config(&config).unwrap_err();
        assert!(err.to_string().contains("Ctrl+Хтоних"), "{err}");
    }
}
