// SPDX-License-Identifier: MIT
//! Global hotkeys
//!
//! On Windows and macOS the GUI itself registers them through `tauri-plugin-global-shortcut`.
//! On Wayland the compositor does not pass the key presses of other programs to the
//! application, so the module is empty there. For Hyprland the GUI writes the binds to a file
//! of their own instead, from the same text as `molva setup hyprland`

use std::path::{Path, PathBuf};

use molva_core::app::hotkeys::hyprland;
use molva_core::config::HotkeysConfig;
use serde::Serialize;

/// The Hyprland binds file as the settings show it: where it is, the line that includes it,
/// and whether it exists yet
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HyprlandBinds {
    pub path: String,
    pub include_line: String,
    pub written: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Where the binds file goes, from the environment Hyprland itself reads
pub fn hyprland_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    hyprland::config_path(xdg.as_deref(), home().as_deref())
}

/// The state of the binds file at `path`
pub fn hyprland_binds(path: &Path) -> HyprlandBinds {
    HyprlandBinds {
        path: path.display().to_string(),
        include_line: hyprland::include_line(path, home().as_deref()),
        written: path.exists(),
    }
}

/// Write the binds. The caller reloads Hyprland: a test must not reload the session it runs in
pub fn write_hyprland_binds(path: &Path, config: &HotkeysConfig) -> Result<HyprlandBinds, String> {
    let text = hyprland::lua(config).map_err(|err| err.to_string())?;
    hyprland::write(path, &text).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(hyprland_binds(path))
}

/// `hyprctl reload`, only inside a Hyprland session; a failure costs the user one manual reload
pub fn reload_hyprland() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    match std::process::Command::new("hyprctl").arg("reload").output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => tracing::warn!(status = %output.status, "hyprctl reload failed"),
        Err(err) => tracing::warn!(%err, "hyprctl reload did not start"),
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

    /// Register the hotkeys from the config. A combination that is already taken does not
    /// crash the start: it goes to the log, and the others register
    pub fn register<R: Runtime>(app: &AppHandle<R>, config: &HotkeysConfig) {
        unregister(app);
        for binding in [
            &config.push_to_talk,
            &config.toggle,
            &config.command,
            &config.cancel,
        ] {
            let Ok(shortcut) = Shortcut::from_str(binding) else {
                tracing::warn!(binding, "cannot parse the key combination");
                continue;
            };
            if let Err(err) = app.global_shortcut().register(shortcut) {
                tracing::warn!(binding, %err, "key combination taken by another application");
            }
        }
    }

    pub fn unregister<R: Runtime>(app: &AppHandle<R>) {
        let _ = app.global_shortcut().unregister_all();
    }

    /// Press and release: a key hold is start/stop, a short press is toggle/cancel
    pub(crate) fn on_shortcut<R: Runtime>(app: &AppHandle<R>, pressed: bool, binding: &str) {
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
            tracing::warn!(%err, "hotkey did not reach the daemon");
        }
    }

    /// Plugin with a handler: the press state arrives in `event.state()`
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
    fn writing_the_binds_reports_the_file_and_its_include_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hypr").join(hyprland::FILE_NAME);
        assert!(!hyprland_binds(&path).written);
        let binds = write_hyprland_binds(&path, &HotkeysConfig::default()).unwrap();
        assert!(binds.written);
        assert!(binds.include_line.starts_with("dofile("), "{binds:?}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, hyprland::lua(&HotkeysConfig::default()).unwrap());
    }

    #[test]
    fn a_hotkey_that_does_not_parse_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(hyprland::FILE_NAME);
        let config = HotkeysConfig {
            toggle: "Ctrl+Telepathy".into(),
            ..HotkeysConfig::default()
        };
        assert!(write_hyprland_binds(&path, &config).is_err());
        assert!(!path.exists());
    }
}
