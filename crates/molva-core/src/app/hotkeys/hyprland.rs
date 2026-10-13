// SPDX-License-Identifier: MIT
//! Hyprland binds for the hotkeys, as a Lua file that the user's `hyprland.lua` includes
//!
//! On Wayland the compositor owns the global hotkeys, so MolvAI cannot grab them itself. Since
//! Hyprland 0.55 its config is Lua, and the hyprlang `hyprland.conf` is deprecated. MolvAI
//! writes its binds to a file of its own and the user includes it with one line, so a change
//! of the hotkeys in the settings rewrites that file and never touches the user's config
//!
//! The GUI and `molva setup` both take the text from here, so the two cannot disagree

use std::io::Write;
use std::path::{Path, PathBuf};

use super::HotkeySpec;
use crate::config::HotkeysConfig;
use crate::domain::hotkeys::HotkeyError;

/// The file name next to the user's `hyprland.lua`
pub const FILE_NAME: &str = "molvai.lua";

/// A bind that one hotkey makes: the command, the description, whether it fires on release, and
/// whether the key still reaches the focused application
struct Bind<'a> {
    keys: &'a str,
    command: &'static str,
    description: &'static str,
    release: bool,
    passes_through: bool,
}

/// The Lua text of the binds. An empty hotkey in the settings means "not assigned" and makes
/// no bind; a hotkey that does not parse is an error, not a bind Hyprland would refuse
pub fn lua(hotkeys: &HotkeysConfig) -> Result<String, HotkeyError> {
    let binds = [
        Bind {
            keys: &hotkeys.push_to_talk,
            command: "molva record start",
            description: "MolvAI: dictate while the key is held",
            release: false,
            passes_through: false,
        },
        // Push-to-talk needs the release as well: it ends the phrase and pastes the text
        Bind {
            keys: &hotkeys.push_to_talk,
            command: "molva record stop",
            description: "MolvAI: paste the text on release",
            release: true,
            passes_through: false,
        },
        Bind {
            keys: &hotkeys.toggle,
            command: "molva record toggle",
            description: "MolvAI: start or stop a recording",
            release: false,
            passes_through: false,
        },
        Bind {
            keys: &hotkeys.command,
            command: "molva record toggle --mode command",
            description: "MolvAI: edit the selected text by voice",
            release: false,
            passes_through: false,
        },
        // Cancel matters only during a recording, and its key usually means something in the
        // application too: a bind on a bare Escape would take Escape away from every program
        Bind {
            keys: &hotkeys.cancel,
            command: "molva record cancel",
            description: "MolvAI: cancel the recording",
            release: false,
            passes_through: true,
        },
    ];
    let mut text = String::from(
        "-- MolvAI hotkeys for Hyprland, written by MolvAI from its settings\n\
         -- Change the hotkeys in MolvAI: it rewrites this file, so an edit here does not last\n",
    );
    for bind in binds {
        if bind.keys.trim().is_empty() {
            continue;
        }
        let spec = HotkeySpec::parse(bind.keys)?;
        let mut flags = String::new();
        if bind.release {
            flags.push_str("release = true, ");
        }
        if bind.passes_through {
            flags.push_str("non_consuming = true, ");
        }
        text.push_str(&format!(
            "hl.bind({}, hl.dsp.exec_cmd({}), {{ {flags}description = {} }})\n",
            lua_string(&combo(&spec)),
            lua_string(bind.command),
            lua_string(bind.description),
        ));
    }
    Ok(text)
}

/// `Ctrl+Alt+Z` becomes `CTRL + ALT + Z`, the key string of `hl.bind`
fn combo(spec: &HotkeySpec) -> String {
    spec.modifiers
        .iter()
        .map(|m| m.label().to_ascii_uppercase())
        .chain(std::iter::once(spec.keysym()))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// A Lua string literal: the path of a file can hold a quote or a backslash
fn lua_string(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Where the file goes: `$XDG_CONFIG_HOME/hypr`, else `~/.config/hypr`, as Hyprland reads it
pub fn config_path(xdg_config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let base = match xdg_config_home {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        _ => home?.join(".config"),
    };
    Some(base.join("hypr").join(FILE_NAME))
}

/// The line that includes the file in `hyprland.lua`
///
/// A path under the home directory is written through `$HOME`, so the line still works in a
/// config that the user keeps in a repository and uses on several machines
pub fn include_line(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(relative) => format!(
            "dofile(os.getenv(\"HOME\") .. {})",
            lua_string(&format!("/{}", relative.display()))
        ),
        None => format!("dofile({})", lua_string(&path.display().to_string())),
    }
}

/// Write the file through a temporary one: Hyprland may read it at any moment, and it must
/// never see half a file
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the path has no directory"))?;
    std::fs::create_dir_all(dir)?;
    let mut temp_name = path.as_os_str().to_owned();
    temp_name.push(".tmp");
    let temp = PathBuf::from(temp_name);
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<&str> {
        text.lines().filter(|l| l.starts_with("hl.bind(")).collect()
    }

    #[test]
    fn push_to_talk_binds_the_press_and_the_release_of_one_key() {
        let config = HotkeysConfig {
            push_to_talk: "F9".into(),
            ..HotkeysConfig::default()
        };
        let text = lua(&config).unwrap();
        let binds = lines(&text);
        let press = binds
            .iter()
            .find(|l| l.contains("\"molva record start\""))
            .unwrap();
        let release = binds
            .iter()
            .find(|l| l.contains("\"molva record stop\""))
            .unwrap();
        assert!(press.starts_with("hl.bind(\"F9\","), "{press}");
        assert!(!press.contains("release = true"), "{press}");
        assert!(release.starts_with("hl.bind(\"F9\","), "{release}");
        assert!(release.contains("release = true"), "{release}");
    }

    #[test]
    fn every_action_gets_its_command_and_its_key_from_the_settings() {
        let config = HotkeysConfig {
            push_to_talk: "Pause".into(),
            toggle: "Ctrl+Alt+Z".into(),
            command: "Super+Shift+x".into(),
            cancel: "Ctrl+Shift+Escape".into(),
            ..HotkeysConfig::default()
        };
        let text = lua(&config).unwrap();
        for (keys, command) in [
            ("CTRL + ALT + Z", "molva record toggle\""),
            ("SHIFT + SUPER + X", "molva record toggle --mode command"),
            ("CTRL + SHIFT + Escape", "molva record cancel"),
        ] {
            assert!(
                lines(&text)
                    .iter()
                    .any(|l| l.starts_with(&format!("hl.bind(\"{keys}\",")) && l.contains(command)),
                "{keys} -> {command}:\n{text}"
            );
        }
    }

    #[test]
    fn only_the_cancel_key_still_reaches_the_application() {
        let config = HotkeysConfig {
            cancel: "Escape".into(),
            ..HotkeysConfig::default()
        };
        let text = lua(&config).unwrap();
        for line in lines(&text) {
            let cancel = line.contains("molva record cancel");
            assert_eq!(line.contains("non_consuming = true"), cancel, "{line}");
        }
    }

    #[test]
    fn an_empty_hotkey_makes_no_bind() {
        let config = HotkeysConfig {
            cancel: String::new(),
            ..HotkeysConfig::default()
        };
        let text = lua(&config).unwrap();
        assert!(!text.contains("molva record cancel"), "{text}");
        assert_eq!(lines(&text).len(), 4, "{text}");
    }

    #[test]
    fn a_hotkey_that_does_not_parse_is_an_error() {
        let config = HotkeysConfig {
            toggle: "Ctrl+Telepathy".into(),
            ..HotkeysConfig::default()
        };
        assert!(lua(&config).is_err());
    }

    #[test]
    fn the_file_goes_to_the_xdg_config_directory_or_to_the_home_config() {
        // Paths that are absolute on every platform: `/cfg` has no drive, so on Windows it is
        // relative and would be ignored as an XDG directory
        let base = std::env::temp_dir();
        let xdg = base.join("cfg");
        let home = base.join("home");
        let in_xdg = xdg.join("hypr").join(FILE_NAME);
        let in_home = home.join(".config").join("hypr").join(FILE_NAME);
        assert_eq!(config_path(Some(&xdg), Some(&home)), Some(in_xdg));
        assert_eq!(config_path(None, Some(&home)), Some(in_home.clone()));
        // A relative XDG_CONFIG_HOME is invalid by the spec and is ignored
        assert_eq!(
            config_path(Some(Path::new("cfg")), Some(&home)),
            Some(in_home)
        );
        assert_eq!(config_path(None, None), None);
    }

    #[test]
    fn the_include_line_goes_through_home_when_it_can() {
        let home = Path::new("/home/u");
        assert_eq!(
            include_line(Path::new("/home/u/.config/hypr/molvai.lua"), Some(home)),
            "dofile(os.getenv(\"HOME\") .. \"/.config/hypr/molvai.lua\")"
        );
        assert_eq!(
            include_line(Path::new("/etc/xdg/hypr/molvai.lua"), Some(home)),
            "dofile(\"/etc/xdg/hypr/molvai.lua\")"
        );
    }

    #[test]
    fn a_quote_in_a_path_cannot_break_the_lua_string() {
        let line = include_line(Path::new("/srv/a\"b\\c/molvai.lua"), None);
        assert_eq!(line, "dofile(\"/srv/a\\\"b\\\\c/molvai.lua\")");
    }

    #[test]
    fn writing_creates_the_directory_and_replaces_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hypr").join(FILE_NAME);
        write(&path, "old").unwrap();
        write(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "temporary files stayed: {leftovers:?}");
    }
}
