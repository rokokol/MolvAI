// SPDX-License-Identifier: MIT
//! Глобальные хоткеи.
//!
//! На Windows и macOS их регистрирует сам GUI через `tauri-plugin-global-shortcut`.
//! На Wayland композитор не отдаёт приложению чужие нажатия, поэтому там модуль пуст,
//! а Settings показывает готовый сниппет биндов для Hyprland — его строит
//! [`hyprland_snippet`], доступный на всех платформах, чтобы текст был один и тот же.

use molva_core::config::HotkeysConfig;

/// Бинды Hyprland для push-to-talk и переключателей: `bind` на нажатие, `bindr` на отпускание.
///
/// Клавиша берётся из настроек как есть: имена клавиш Hyprland (`Pause`, `SUPER, Z`)
/// пользователь пишет сам, GUI их не переводит.
pub fn hyprland_snippet(config: &HotkeysConfig) -> String {
    let ptt = hypr_key(&config.push_to_talk);
    let toggle = hypr_key(&config.toggle);
    let command = hypr_key(&config.command);
    let cancel = hypr_key(&config.cancel);
    format!(
        "# MolvAI: удержание клавиши — диктовка, отпускание — вставка текста\n\
         bind  = {ptt}, exec, molva record start\n\
         bindr = {ptt}, exec, molva record stop\n\
         # переключатель, командный режим и отмена\n\
         bind  = {toggle}, exec, molva record toggle\n\
         bind  = {command}, exec, molva record toggle --mode command\n\
         bind  = {cancel}, exec, molva record cancel\n"
    )
}

/// `Ctrl+Shift+Space` → `CTRL SHIFT, Space`; одиночная клавиша → `, Pause`.
fn hypr_key(binding: &str) -> String {
    let parts: Vec<&str> = binding
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return String::from(", Pause");
    };
    let mods: Vec<String> = modifiers.iter().map(|m| hypr_modifier(m)).collect();
    format!("{}, {}", mods.join(" "), key)
}

fn hypr_modifier(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "leftctrl" | "rightctrl" => "CTRL".into(),
        "shift" | "leftshift" | "rightshift" => "SHIFT".into(),
        "alt" | "leftalt" | "rightalt" => "ALT".into(),
        "super" | "meta" | "win" | "cmd" => "SUPER".into(),
        other => other.to_ascii_uppercase(),
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::str::FromStr;

    use molva_core::config::HotkeysConfig;
    use molva_core::domain::entry::Mode;
    use molva_core::ipc::Command;
    use tauri::{AppHandle, Manager, Runtime};
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

    use crate::commands::AppState;

    /// Зарегистрировать хоткеи из конфига. Уже занятая комбинация не роняет запуск:
    /// она попадает в лог, а остальные регистрируются.
    pub fn register<R: Runtime>(app: &AppHandle<R>, config: &HotkeysConfig) {
        unregister(app);
        for binding in [
            &config.push_to_talk,
            &config.toggle,
            &config.command,
            &config.cancel,
        ] {
            let Ok(shortcut) = Shortcut::from_str(binding) else {
                tracing::warn!(binding, "не удалось разобрать сочетание клавиш");
                continue;
            };
            if let Err(err) = app.global_shortcut().register(shortcut) {
                tracing::warn!(binding, %err, "сочетание клавиш занято другим приложением");
            }
        }
    }

    pub fn unregister<R: Runtime>(app: &AppHandle<R>) {
        let _ = app.global_shortcut().unregister_all();
    }

    /// Нажатие и отпускание: удержание — это start/stop, короткие — toggle/cancel.
    pub fn on_shortcut<R: Runtime>(app: &AppHandle<R>, pressed: bool, binding: &str) {
        let state = app.state::<AppState>();
        if state.hotkeys_paused() {
            return;
        }
        let config = state.config();
        let hotkeys = &config.hotkeys;
        let command = if binding == hotkeys.push_to_talk {
            if pressed {
                Command::RecordStart {
                    mode: Mode::Dictation,
                    style: None,
                }
            } else {
                Command::RecordStop
            }
        } else if !pressed {
            return;
        } else if binding == hotkeys.toggle {
            Command::RecordToggle {
                mode: Mode::Dictation,
                style: None,
            }
        } else if binding == hotkeys.command {
            Command::RecordToggle {
                mode: Mode::Command,
                style: None,
            }
        } else if binding == hotkeys.cancel {
            Command::RecordCancel
        } else {
            return;
        };
        if let Err(err) = crate::ipc::request(command) {
            tracing::warn!(%err, "хоткей не дошёл до демона");
        }
    }

    /// Плагин с обработчиком: состояние нажатия приходит в `event.state()`.
    pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, shortcut, event| {
                on_shortcut(
                    app,
                    matches!(event.state(), ShortcutState::Pressed),
                    &shortcut.into_string(),
                );
            })
            .build()
    }
}

#[cfg(not(target_os = "linux"))]
pub use platform::{plugin, register, unregister};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_key_binding_has_an_empty_modifier_field() {
        assert_eq!(hypr_key("Pause"), ", Pause");
    }

    #[test]
    fn modifiers_become_hyprland_names() {
        assert_eq!(hypr_key("Ctrl+Shift+Space"), "CTRL SHIFT, Space");
        assert_eq!(hypr_key("Super+Z"), "SUPER, Z");
    }

    #[test]
    fn empty_binding_falls_back_to_a_usable_key() {
        assert_eq!(hypr_key(""), ", Pause");
    }

    #[test]
    fn snippet_binds_press_and_release_of_push_to_talk() {
        let config = HotkeysConfig {
            push_to_talk: "Pause".into(),
            ..HotkeysConfig::default()
        };
        let snippet = hyprland_snippet(&config);
        assert!(snippet.contains("bind  = , Pause, exec, molva record start"));
        assert!(snippet.contains("bindr = , Pause, exec, molva record stop"));
        assert!(snippet.contains("molva record cancel"));
    }
}
