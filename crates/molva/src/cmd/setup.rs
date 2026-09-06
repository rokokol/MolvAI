// SPDX-License-Identifier: MIT
//! `molva setup` — готовые бинды для композитора.
//!
//! На Wayland глобальные хоткеи выдаёт композитор, а не приложение, поэтому «настройка» — это
//! строки в конфиге пользователя. Команда печатает их, а не правит чужие файлы: конфиг
//! композитора — не наша территория.

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

/// Комбинация, разобранная на модификаторы и клавишу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Combo {
    pub mods: Vec<String>,
    pub key: String,
}

impl Combo {
    /// Разбор `Ctrl+Shift+Space`; последняя часть — клавиша, остальные — модификаторы.
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

    /// `CTRL SHIFT` для Hyprland; пустая строка, если модификаторов нет.
    fn hyprland_mods(&self) -> String {
        self.mods
            .iter()
            .map(|m| m.to_uppercase())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `Ctrl+Shift+space` для Sway.
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

/// Имя клавиши в терминах xkb: композиторы понимают именно их.
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

/// Сниппет для композитора. `ptt` переопределяет клавишу push-to-talk из конфига.
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
         # Push-to-talk: bind срабатывает на нажатие, bindr — на отпускание.\n\
         bind  = {ptt_mods}, {ptt_key}, exec, molva record start\n\
         bindr = {ptt_mods}, {ptt_key}, exec, molva record stop\n\
         \n\
         # Hands-free: одно нажатие включает запись, следующее — выключает.\n\
         bind  = {toggle_mods}, {toggle_key}, exec, molva record toggle\n\
         \n\
         # Режим команд: голосовая правка выделенного текста.\n\
         bind  = {command_mods}, {command_key}, exec, molva record start --mode command\n\
         bindr = {command_mods}, {command_key}, exec, molva record stop\n\
         \n\
         # Отмена текущей записи.\n\
         bind  = CTRL SHIFT, Escape, exec, molva record cancel\n\
         \n\
         # Демон должен быть запущен: molva daemon\n\
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
         # --no-repeat не даёт автоповтору начинать запись заново.\n\
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
         # Plasma не отдаёт отпускание клавиши, поэтому push-to-talk там недоступен:\n\
         # рабочий режим — переключатель.\n\
         #\n\
         # Параметры системы → Комбинации клавиш → Добавить команду:\n\
         #   команда:     molva record toggle\n\
         #   комбинация:  {toggle}\n\
         #\n\
         # Отмена:\n\
         #   команда:     molva record cancel\n\
         #   комбинация:  Ctrl+Shift+Escape\n\
         #\n\
         # Автозапуск демона: Параметры системы → Автозапуск → molva daemon\n",
        toggle = toggle.sway_combo(),
    )
}

fn gnome(toggle: &Combo) -> String {
    let path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/molva/";
    format!(
        "# MolvAI — GNOME\n\
         # GNOME тоже не отдаёт отпускание клавиши: только переключатель.\n\
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

// Общая для всех подкоманд сигнатура: диспетчер в `main` вызывает их одинаково.
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
            "push-to-talk там не работает: {kde}"
        );
        let gnome = snippet(Target::Gnome, &HotkeysConfig::default(), None);
        assert!(gnome.contains("gsettings set"), "{gnome}");
        assert!(gnome.contains("molva record toggle"), "{gnome}");
    }

    #[test]
    fn the_ptt_flag_overrides_the_config() {
        let text = snippet(Target::Hyprland, &HotkeysConfig::default(), Some("Pause"));
        assert!(text.contains(", Pause, exec, molva record start"), "{text}");
        // Умолчание — F9: нарочно простая клавиша, которая свободна на любом стенде.
        let default = snippet(Target::Hyprland, &HotkeysConfig::default(), None);
        assert!(default.contains(", F9, exec, molva record start"), "{default}");
    }

    #[test]
    fn target_names_are_parsed_and_unknown_ones_are_refused() {
        assert_eq!(Target::parse("Hyprland"), Some(Target::Hyprland));
        assert_eq!(Target::parse("plasma"), Some(Target::Kde));
        assert_eq!(Target::parse("i3"), None);
    }
}
