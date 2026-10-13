// SPDX-License-Identifier: MIT
//! `molva setup` — ready binds for the compositor
//!
//! On Wayland the compositor gives out the global hotkeys, not the application, so the "setup" is
//! lines in the config of the user. The command prints them and never edits the user's config.
//! For Hyprland it can write the binds to a file of their own (`--write`), which the user's
//! `hyprland.lua` includes with one line: the GUI writes the same file from the same text

use std::path::{Path, PathBuf};

use molva_core::app::hotkeys::{hyprland, HotkeySpec};
use molva_core::config::HotkeysConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Hyprland,
    Sway,
    Kde,
    Gnome,
}

impl Target {
    pub(crate) fn parse(value: &str) -> Option<Target> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hyprland" => Some(Target::Hyprland),
            "sway" => Some(Target::Sway),
            "kde" | "plasma" => Some(Target::Kde),
            "gnome" => Some(Target::Gnome),
            _ => None,
        }
    }
}

/// `Ctrl+Shift+space`: the combination as Sway and Plasma write it
fn plus_combo(spec: &HotkeySpec) -> String {
    spec.modifiers
        .iter()
        .map(|m| m.label().to_string())
        .chain(std::iter::once(spec.keysym()))
        .collect::<Vec<_>>()
        .join("+")
}

/// `<Ctrl><Alt>Z`: the combination as GNOME writes it
fn gnome_combo(spec: &HotkeySpec) -> String {
    let mut combo = String::new();
    for modifier in &spec.modifiers {
        combo.push('<');
        combo.push_str(modifier.label());
        combo.push('>');
    }
    combo.push_str(&spec.keysym());
    combo
}

/// The hotkeys with the push-to-talk key from `--ptt`, when it is given
fn with_ptt(hotkeys: &HotkeysConfig, ptt: Option<&str>) -> HotkeysConfig {
    let mut hotkeys = hotkeys.clone();
    if let Some(ptt) = ptt {
        hotkeys.push_to_talk = ptt.to_string();
    }
    hotkeys
}

/// The snippet for a compositor
pub(crate) fn snippet(
    target: Target,
    hotkeys: &HotkeysConfig,
    ptt: Option<&str>,
) -> anyhow::Result<String> {
    let hotkeys = with_ptt(hotkeys, ptt);
    Ok(match target {
        Target::Hyprland => format!(
            "{}\n\
             -- Without the GUI, start the daemon with Hyprland:\n\
             -- hl.on(\"hyprland.start\", function() hl.exec_cmd(\"molva daemon\") end)\n",
            hyprland::lua(&hotkeys)?
        ),
        Target::Sway => sway(&hotkeys)?,
        Target::Kde => kde(&HotkeySpec::parse(&hotkeys.toggle)?),
        Target::Gnome => gnome(&HotkeySpec::parse(&hotkeys.toggle)?),
    })
}

fn sway(hotkeys: &HotkeysConfig) -> anyhow::Result<String> {
    let ptt = plus_combo(&HotkeySpec::parse(&hotkeys.push_to_talk)?);
    let toggle = plus_combo(&HotkeySpec::parse(&hotkeys.toggle)?);
    let command = plus_combo(&HotkeySpec::parse(&hotkeys.command)?);
    let cancel = plus_combo(&HotkeySpec::parse(&hotkeys.cancel)?);
    Ok(format!(
        "# MolvAI — ~/.config/sway/config\n\
         # --no-repeat stops the auto-repeat from starting the recording again\n\
         bindsym --no-repeat {ptt} exec molva record start\n\
         bindsym --release {ptt} exec molva record stop\n\
         \n\
         bindsym --no-repeat {toggle} exec molva record toggle\n\
         bindsym --no-repeat {command} exec molva record toggle --mode command\n\
         bindsym --no-repeat {cancel} exec molva record cancel\n\
         \n\
         exec molva daemon\n"
    ))
}

fn kde(toggle: &HotkeySpec) -> String {
    format!(
        "# MolvAI — KDE Plasma\n\
         # Plasma does not give out the key release, so push-to-talk is unavailable there:\n\
         # the working mode is the toggle\n\
         #\n\
         # System Settings → Shortcuts → Add Command:\n\
         #   command:     molva record toggle\n\
         #   shortcut:    {toggle}\n\
         #\n\
         # Autostart of the daemon: System Settings → Autostart → molva daemon\n",
        toggle = plus_combo(toggle),
    )
}

fn gnome(toggle: &HotkeySpec) -> String {
    let path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/molva/";
    format!(
        "# MolvAI — GNOME\n\
         # GNOME does not give out the key release either: only the toggle\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings \\\n\
         \x20   \"['{path}']\"\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{path} \\\n\
         \x20   name 'MolvAI toggle'\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{path} \\\n\
         \x20   command 'molva record toggle'\n\
         gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{path} \\\n\
         \x20   binding '{binding}'\n",
        binding = gnome_combo(toggle),
    )
}

/// Where `--write` puts the Hyprland file, from the environment Hyprland itself reads
pub(crate) fn hyprland_path() -> anyhow::Result<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    hyprland::config_path(xdg.as_deref(), home.as_deref())
        .ok_or_else(|| anyhow::anyhow!("neither XDG_CONFIG_HOME nor HOME is set"))
}

/// Write the Hyprland file and say which line includes it
pub(crate) fn write_hyprland(
    hotkeys: &HotkeysConfig,
    ptt: Option<&str>,
    path: &Path,
) -> anyhow::Result<String> {
    hyprland::write(path, &hyprland::lua(&with_ptt(hotkeys, ptt))?)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Ok(format!(
        "wrote {}\nadd this line to hyprland.lua once:\n{}\n",
        path.display(),
        hyprland::include_line(path, home.as_deref())
    ))
}

pub(crate) fn run(
    target: Target,
    hotkeys: &HotkeysConfig,
    ptt: Option<&str>,
    write: bool,
) -> anyhow::Result<()> {
    if write {
        if target != Target::Hyprland {
            anyhow::bail!("--write is for Hyprland only: the other compositors keep binds in their own config");
        }
        print!("{}", write_hyprland(hotkeys, ptt, &hyprland_path()?)?);
        return Ok(());
    }
    print!("{}", snippet(target, hotkeys, ptt)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> HotkeySpec {
        HotkeySpec::parse(text).unwrap()
    }

    #[test]
    fn combinations_take_each_compositor_spelling() {
        assert_eq!(plus_combo(&spec("Ctrl+Shift+Space")), "Ctrl+Shift+space");
        assert_eq!(plus_combo(&spec("F9")), "F9");
        assert_eq!(gnome_combo(&spec("Ctrl+Alt+z")), "<Ctrl><Alt>Z");
    }

    #[test]
    fn the_hyprland_snippet_is_the_lua_file_of_the_core() {
        let hotkeys = HotkeysConfig::default();
        let text = snippet(Target::Hyprland, &hotkeys, None).unwrap();
        assert!(
            text.starts_with(&hyprland::lua(&hotkeys).unwrap()),
            "{text}"
        );
        assert!(text.contains("molva daemon"), "{text}");
    }

    #[test]
    fn the_ptt_flag_overrides_the_config() {
        let text = snippet(Target::Hyprland, &HotkeysConfig::default(), Some("Pause")).unwrap();
        assert!(text.contains("hl.bind(\"Pause\""), "{text}");
        // The default is F9: a deliberately simple key that is free on any test bench
        let default = snippet(Target::Hyprland, &HotkeysConfig::default(), None).unwrap();
        assert!(default.contains("hl.bind(\"F9\""), "{default}");
    }

    #[test]
    fn the_sway_snippet_takes_every_key_from_the_settings() {
        let hotkeys = HotkeysConfig {
            cancel: "Ctrl+Alt+C".into(),
            ..HotkeysConfig::default()
        };
        let text = snippet(Target::Sway, &hotkeys, Some("F9")).unwrap();
        assert!(
            text.contains("bindsym --no-repeat F9 exec molva record start"),
            "{text}"
        );
        assert!(
            text.contains("bindsym --release F9 exec molva record stop"),
            "{text}"
        );
        assert!(
            text.contains("Ctrl+Alt+C exec molva record cancel"),
            "{text}"
        );
        assert!(
            text.contains("molva record toggle --mode command"),
            "{text}"
        );
    }

    #[test]
    fn kde_and_gnome_offer_the_toggle_with_its_own_modifiers() {
        let hotkeys = HotkeysConfig {
            toggle: "Super+Z".into(),
            ..HotkeysConfig::default()
        };
        let kde = snippet(Target::Kde, &hotkeys, None).unwrap();
        assert!(kde.contains("Super+Z"), "{kde}");
        assert!(
            !kde.contains("record start"),
            "push-to-talk does not work there: {kde}"
        );
        let gnome = snippet(Target::Gnome, &hotkeys, None).unwrap();
        assert!(gnome.contains("binding '<Super>Z'"), "{gnome}");
    }

    #[test]
    fn a_hotkey_that_does_not_parse_is_reported() {
        let hotkeys = HotkeysConfig {
            toggle: "Ctrl+Telepathy".into(),
            ..HotkeysConfig::default()
        };
        assert!(snippet(Target::Sway, &hotkeys, None).is_err());
        assert!(snippet(Target::Hyprland, &hotkeys, None).is_err());
    }

    #[test]
    fn write_puts_the_file_in_place_and_names_the_include_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hypr").join(hyprland::FILE_NAME);
        let report = write_hyprland(&HotkeysConfig::default(), None, &path).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, hyprland::lua(&HotkeysConfig::default()).unwrap());
        assert!(report.contains("dofile("), "{report}");
    }

    #[test]
    fn target_names_are_parsed_and_unknown_ones_are_refused() {
        assert_eq!(Target::parse("Hyprland"), Some(Target::Hyprland));
        assert_eq!(Target::parse("plasma"), Some(Target::Kde));
        assert_eq!(Target::parse("i3"), None);
    }
}
