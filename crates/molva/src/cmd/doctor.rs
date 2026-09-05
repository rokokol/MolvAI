// SPDX-License-Identifier: MIT
//! `molva doctor` — что в этой системе работает, а что нет и почему.
//!
//! Диагностика печатает не «ок/не ок», а причину и следующий шаг: пользователь на NixOS без
//! правила udev должен из вывода понять, что именно ему добавить.

use std::path::Path;

use molva_core::infra::inject::clipboard::SystemClipboard;
use molva_core::infra::ipc;
use molva_core::infra::platform::{self, Platform, Tools};

/// Одна строка отчёта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn new(name: &str, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }

    pub(crate) fn line(&self) -> String {
        let mark = if self.ok { "да" } else { "нет" };
        format!("{:<24} {:<4} {}", self.name, mark, self.detail)
    }
}

/// Доступность `/dev/uinput` на запись: файл может существовать и быть закрытым.
pub(crate) fn uinput_check() -> Check {
    let path = Path::new("/dev/uinput");
    if !path.exists() {
        return Check::new("/dev/uinput", false, "нет модуля uinput в ядре");
    }
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => Check::new("/dev/uinput", true, "открывается на запись"),
        Err(err) => Check::new(
            "/dev/uinput",
            false,
            format!("{err}; нужно правило udev на группу input"),
        ),
    }
}

/// Клавиатуры в `/dev/input`, которые нам разрешено читать.
pub(crate) fn input_devices_check() -> Check {
    #[cfg(target_os = "linux")]
    {
        use molva_core::infra::hotkeys::evdev_source::EvdevHotkeys;
        let devices = EvdevHotkeys::devices();
        if devices.is_empty() {
            return Check::new(
                "/dev/input",
                false,
                "клавиатуры не читаются: добавьте пользователя в группу input",
            );
        }
        Check::new("/dev/input", true, format!("клавиатур: {}", devices.len()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Check::new("/dev/input", false, "только Linux")
    }
}

/// Полный отчёт: строки в том порядке, в котором их читают.
pub(crate) fn checks(socket: &Path) -> Vec<Check> {
    let platform = platform::detect();
    let tools = Tools::detect();
    let mut checks = vec![
        Check::new("сессия", platform != Platform::Headless, platform.label()),
        Check::new(
            "окно",
            true,
            platform::active_window_class().unwrap_or_else(|| "класс неизвестен".into()),
        ),
        Check::new("hyprctl", tools.hyprctl, tool_detail(tools.hyprctl)),
        Check::new("wtype", tools.wtype, tool_detail(tools.wtype)),
        Check::new("ydotool", tools.ydotool, tool_detail(tools.ydotool)),
        Check::new("wl-copy", tools.wl_copy, tool_detail(tools.wl_copy)),
        Check::new(
            "буфер обмена",
            SystemClipboard::available(),
            "arboard или wl-copy",
        ),
        uinput_check(),
        input_devices_check(),
    ];
    checks.push(match ipc::ping(socket) {
        Some(pid) => Check::new("демон", true, format!("pid {pid}, {}", socket.display())),
        None => Check::new(
            "демон",
            false,
            format!(
                "не отвечает на {}; запустите molva daemon",
                socket.display()
            ),
        ),
    });
    checks
}

fn tool_detail(found: bool) -> &'static str {
    if found {
        "найден в PATH"
    } else {
        "не найден в PATH"
    }
}

pub(crate) fn run(socket: &Path) -> anyhow::Result<()> {
    for check in checks(socket) {
        println!("{}", check.line());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_line_shows_the_name_the_verdict_and_the_reason() {
        let check = Check::new("wtype", false, "не найден в PATH");
        let line = check.line();
        assert!(line.starts_with("wtype"), "{line}");
        assert!(line.contains("нет"), "{line}");
        assert!(line.contains("не найден"), "{line}");
    }

    #[test]
    fn the_report_covers_session_tools_and_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let checks = checks(&dir.path().join("absent.sock"));
        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        for expected in ["сессия", "hyprctl", "wtype", "/dev/uinput", "демон"] {
            assert!(names.contains(&expected), "{names:?}");
        }
    }

    #[test]
    fn a_missing_daemon_is_reported_with_the_socket_path_and_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let checks = checks(&socket);
        let daemon = checks.iter().find(|c| c.name == "демон").unwrap();
        assert!(!daemon.ok);
        assert!(daemon.detail.contains("molva daemon"), "{}", daemon.detail);
        assert!(
            daemon.detail.contains(&socket.display().to_string()),
            "{}",
            daemon.detail
        );
    }

    #[test]
    fn uinput_check_never_panics_and_always_explains_itself() {
        let check = uinput_check();
        assert!(!check.detail.is_empty());
    }
}
