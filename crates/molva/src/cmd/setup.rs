// SPDX-License-Identifier: MIT
//! `molva setup` — ready binds for the compositor
//!
//! On Wayland the compositor gives out the global hotkeys, not the application, so the "setup" is
//! lines in the config of the user. The command prints them and does not edit foreign files: the config
//! of the compositor is not our territory

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

/// A combination parsed into modifiers and a key
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Combo {
    pub mods: Vec<String>,
    pub key: String,
}

impl Combo {
    /// Parse `Ctrl+Shift+Space`; the last part is the key, the others are modifiers
    pub(crate) fn parse(spec: &str) -> Combo {
        let parts: Vec<&str> = spec
            .split(['+', '-'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let (key, mods) = parts
            .split_last()
            .map_or(("", &[] as &[&str]), |(k, m)| (*k, m));
        Combo {
            mods: mods.iter().map(|m| normalize_mod(m)).collect(),
            key: key.to_string(),
        }
    }

    /// `CTRL SHIFT` for Hyprland; an empty string if there are no modifiers
    fn hyprland_mods(&self) -> String {
        self.mods
            .iter()
            .map(|m| m.to_uppercase())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `Ctrl+Shift+space` for Sway
    fn sway_combo(&self) -> String {
        let mut parts = self.mods.clone();
        parts.push(keysym(&self.key));
        parts.join("+")
    }
}

fn normalize_mod(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => "Ctrl".into(),
        "shift" => "Shift".into(),
        "alt" | "option" => "Alt".into(),
        "super" | "meta" | "win" | "cmd" => "Super".into(),
        other => other.to_string(),
    }
}

/// The key name in xkb terms: the compositors understand exactly these
fn keysym(key: &str) -> String {
    let key = key.trim();
    if key.len() == 1 {
        return key.to_uppercase();
    }
    let lower = key.to_ascii_lowercase();
    if lower.starts_with('f') && lower[1..].chars().all(|c| c.is_ascii_digit()) {
        return key.to_uppercase();
    }
    match lower.as_str() {
        "space" => "space".into(),
        "escape" | "esc" => "Escape".into(),
        "enter" | "return" => "Return".into(),
        "tab" => "Tab".into(),
        "pause" => "Pause".into(),
        "rightctrl" => "Control_R".into(),
        "leftctrl" => "Control_L".into(),
        "rightalt" => "Alt_R".into(),
        "leftalt" => "Alt_L".into(),
        "rightshift" => "Shift_R".into(),
        "leftshift" => "Shift_L".into(),
        other => other.to_string(),
    }
}

/// The snippet for a compositor. `ptt` overrides the push-to-talk key from the config
pub(crate) fn snippet(target: Target, hotkeys: &HotkeysConfig, ptt: Option<&str>) -> String {
    let ptt = Combo::parse(ptt.unwrap_or(&hotkeys.push_to_talk));
    let toggle = Combo::parse(&hotkeys.toggle);
    let command = Combo::parse(&hotkeys.command);
    match target {
        Target::Hyprland => hyprland(&ptt, &toggle, &command),
        Target::Sway => sway(&ptt, &toggle, &command),
        Target::Kde => kde(&toggle),
        Target::Gnome => gnome(&toggle),
    }
}

fn hyprland(ptt: &Combo, toggle: &Combo, command: &Combo) -> String {
    format!(
        "# MolvAI — ~/.config/hypr/hyprland.conf\n\
         # Push-to-talk: bind fires on press, bindr fires on release\n\
         bind  = {ptt_mods}, {ptt_key}, exec, molva record start\n\
         bindr = {ptt_mods}, {ptt_key}, exec, molva record stop\n\
         \n\
         # Hands-free: one press starts the recording, the next one stops it\n\
         bind  = {toggle_mods}, {toggle_key}, exec, molva record toggle\n\
         \n\
         # Command mode: voice editing of the selected text\n\
         bind  = {command_mods}, {command_key}, exec, molva record start --mode command\n\
         bindr = {command_mods}, {command_key}, exec, molva record stop\n\
         \n\
         # Cancel the current recording\n\
         bind  = CTRL SHIFT, Escape, exec, molva record cancel\n\
         \n\
         # The daemon must be running: molva daemon\n\
         exec-once = molva daemon\n",
        ptt_mods = ptt.hyprland_mods(),
        ptt_key = keysym(&ptt.key),
        toggle_mods = toggle.hyprland_mods(),
        toggle_key = keysym(&toggle.key),
        command_mods = command.hyprland_mods(),
        command_key = keysym(&command.key),
    )
}

fn sway(ptt: &Combo, toggle: &Combo, command: &Combo) -> String {
    format!(
        "# MolvAI — ~/.config/sway/config\n\
         # --no-repeat stops the auto-repeat from starting the recording again\n\
         bindsym --no-repeat {ptt} exec molva record start\n\
         bindsym --release {ptt} exec molva record stop\n\
         \n\
         bindsym --no-repeat {toggle} exec molva record toggle\n\
         \n\
         bindsym --no-repeat {command} exec molva record start --mode command\n\
         bindsym --release {command} exec molva record stop\n\
         \n\
         bindsym --no-repeat Ctrl+Shift+Escape exec molva record cancel\n\
         \n\
         exec molva daemon\n",
        ptt = ptt.sway_combo(),
        toggle = toggle.sway_combo(),
        command = command.sway_combo(),
    )
}

fn kde(toggle: &Combo) -> String {
    format!(
        "# MolvAI — KDE Plasma\n\
         # Plasma does not give out the key release, so push-to-talk is unavailable there:\n\
         # the working mode is the toggle\n\
         #\n\
         # System Settings → Shortcuts → Add Command:\n\
         #   command:     molva record toggle\n\
         #   shortcut:    {toggle}\n\
         #\n\
         # Cancel:\n\
         #   command:     molva record cancel\n\
         #   shortcut:    Ctrl+Shift+Escape\n\
         #\n\
         # Autostart of the daemon: System Settings → Autostart → molva daemon\n",
        toggle = toggle.sway_combo(),
    )
}

fn gnome(toggle: &Combo) -> String {
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
         \x20   binding '<Ctrl><Shift>{key}'\n",
        path = path,
        key = keysym(&toggle.key),
    )
}

// A signature shared by all subcommands: the dispatcher in `main` calls them all the same way
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn run(
    target: Target,
    hotkeys: &HotkeysConfig,
    ptt: Option<&str>,
) -> anyhow::Result<()> {
    print!("{}", snippet(target, hotkeys, ptt));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_combination_splits_into_modifiers_and_a_key() {
        let combo = Combo::parse("Ctrl+Shift+Space");
        assert_eq!(combo.mods, vec!["Ctrl".to_string(), "Shift".to_string()]);
        assert_eq!(combo.key, "Space");
        assert_eq!(combo.hyprland_mods(), "CTRL SHIFT");
        assert_eq!(combo.sway_combo(), "Ctrl+Shift+space");
    }

    #[test]
    fn a_lone_key_has_no_modifiers() {
        let combo = Combo::parse("F9");
        assert!(combo.mods.is_empty());
        assert_eq!(combo.hyprland_mods(), "");
        assert_eq!(combo.sway_combo(), "F9");
    }

    #[test]
    fn key_names_become_xkb_keysyms() {
        assert_eq!(keysym("Space"), "space");
        assert_eq!(keysym("escape"), "Escape");
        assert_eq!(keysym("RightCtrl"), "Control_R");
        assert_eq!(keysym("f9"), "F9");
        assert_eq!(keysym("s"), "S");
    }

    #[test]
    fn the_hyprland_snippet_binds_both_press_and_release() {
        let text = snippet(Target::Hyprland, &HotkeysConfig::default(), Some("F9"));
        assert!(
            text.contains("bind  = , F9, exec, molva record start"),
            "{text}"
        );
        assert!(
            text.contains("bindr = , F9, exec, molva record stop"),
            "{text}"
        );
        assert!(text.contains("molva record toggle"), "{text}");
        assert!(text.contains("--mode command"), "{text}");
        assert!(text.contains("molva record cancel"), "{text}");
    }

    #[test]
    fn the_sway_snippet_disables_key_repeat_on_press() {
        let text = snippet(Target::Sway, &HotkeysConfig::default(), Some("F9"));
        assert!(
            text.contains("bindsym --no-repeat F9 exec molva record start"),
            "{text}"
        );
        assert!(
            text.contains("bindsym --release F9 exec molva record stop"),
            "{text}"
        );
    }

    #[test]
    fn kde_and_gnome_offer_a_toggle_because_they_hide_key_release() {
        let kde = snippet(Target::Kde, &HotkeysConfig::default(), None);
        assert!(kde.contains("molva record toggle"), "{kde}");
        assert!(
            !kde.contains("record start"),
            "push-to-talk does not work there: {kde}"
        );
        let gnome = snippet(Target::Gnome, &HotkeysConfig::default(), None);
        assert!(gnome.contains("gsettings set"), "{gnome}");
        assert!(gnome.contains("molva record toggle"), "{gnome}");
    }

    #[test]
    fn the_ptt_flag_overrides_the_config() {
        let text = snippet(Target::Hyprland, &HotkeysConfig::default(), Some("Pause"));
        assert!(text.contains(", Pause, exec, molva record start"), "{text}");
        // The default is F9: a deliberately simple key that is free on any test bench
        let default = snippet(Target::Hyprland, &HotkeysConfig::default(), None);
        assert!(
            default.contains(", F9, exec, molva record start"),
            "{default}"
        );
    }

    #[test]
    fn target_names_are_parsed_and_unknown_ones_are_refused() {
        assert_eq!(Target::parse("Hyprland"), Some(Target::Hyprland));
        assert_eq!(Target::parse("plasma"), Some(Target::Kde));
        assert_eq!(Target::parse("i3"), None);
    }
}
