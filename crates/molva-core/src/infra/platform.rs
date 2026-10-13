// SPDX-License-Identifier: MIT
//! Detection of the platform and of the active window
//!
//! The order of the paste methods depends fully on where the daemon runs, so session detection
//! is not diagnostics but a part of the logic. It reads the environment once and stores it in
//! `SessionEnv`, so tests can check the rules without `std::env::set_var`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compositor {
    Hyprland,
    Sway,
    Kde,
    Gnome,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "compositor")]
pub enum Platform {
    X11,
    Wayland(Compositor),
    Windows,
    MacOs,
    /// No session: the daemon runs in a tty, in a container or over ssh
    Headless,
}

impl Platform {
    pub fn is_wayland(self) -> bool {
        matches!(self, Platform::Wayland(_))
    }

    pub fn compositor(self) -> Option<Compositor> {
        match self {
            Platform::Wayland(c) => Some(c),
            _ => None,
        }
    }

    /// Short name for `doctor` and the journal
    pub fn label(self) -> String {
        match self {
            Platform::X11 => "X11".into(),
            Platform::Wayland(c) => format!("Wayland/{c:?}"),
            Platform::Windows => "Windows".into(),
            Platform::MacOs => "macOS".into(),
            Platform::Headless => "no graphical session".into(),
        }
    }
}

/// Snapshot of the environment variables that decide the session
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionEnv {
    pub session_type: Option<String>,
    pub hyprland_signature: Option<String>,
    pub swaysock: Option<String>,
    pub current_desktop: Option<String>,
    pub wayland_display: Option<String>,
    pub x11_display: Option<String>,
}

impl SessionEnv {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            session_type: var("XDG_SESSION_TYPE"),
            hyprland_signature: var("HYPRLAND_INSTANCE_SIGNATURE"),
            swaysock: var("SWAYSOCK"),
            current_desktop: var("XDG_CURRENT_DESKTOP"),
            wayland_display: var("WAYLAND_DISPLAY"),
            x11_display: var("DISPLAY"),
        }
    }
}

/// Detect the platform from the environment of the current process
pub fn detect() -> Platform {
    detect_from(&SessionEnv::from_env())
}

/// The same, but from an explicit environment snapshot: tests check the rules this way
pub fn detect_from(env: &SessionEnv) -> Platform {
    if cfg!(target_os = "windows") {
        return Platform::Windows;
    }
    if cfg!(target_os = "macos") {
        return Platform::MacOs;
    }
    let session = env.session_type.as_deref().unwrap_or("");
    let wayland = session.eq_ignore_ascii_case("wayland") || env.wayland_display.is_some();
    if wayland {
        return Platform::Wayland(compositor_from(env));
    }
    if session.eq_ignore_ascii_case("x11") || env.x11_display.is_some() {
        return Platform::X11;
    }
    Platform::Headless
}

fn compositor_from(env: &SessionEnv) -> Compositor {
    if env.hyprland_signature.is_some() {
        return Compositor::Hyprland;
    }
    if env.swaysock.is_some() {
        return Compositor::Sway;
    }
    let desktop = env
        .current_desktop
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    // XDG_CURRENT_DESKTOP can be a colon-separated list, for example `KDE:wayland`
    for part in desktop.split(':') {
        match part {
            "hyprland" => return Compositor::Hyprland,
            "sway" => return Compositor::Sway,
            "kde" | "plasma" => return Compositor::Kde,
            "gnome" | "ubuntu:gnome" => return Compositor::Gnome,
            _ => {}
        }
    }
    Compositor::Other
}

/// Class of the active window, for the style hint and for the choice of the paste key combination
///
/// It is known only where the compositor reports it; elsewhere it is `None`, and this is normal
pub fn active_window_class() -> Option<String> {
    match detect() {
        Platform::Wayland(Compositor::Hyprland) => hyprland_active_class(),
        _ => None,
    }
}

fn hyprland_active_class() -> Option<String> {
    let output = std::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    json.get("class")?
        .as_str()
        .map(str::to_string)
        .filter(|c| !c.is_empty())
}

/// External utilities found in `PATH`: for `doctor` and for the choice of the chain
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tools {
    pub hyprctl: bool,
    pub wtype: bool,
    pub ydotool: bool,
    pub wl_copy: bool,
    pub xdotool: bool,
}

impl Tools {
    pub fn detect() -> Self {
        Self {
            hyprctl: which::which("hyprctl").is_ok(),
            wtype: which::which("wtype").is_ok(),
            ydotool: which::which("ydotool").is_ok(),
            wl_copy: which::which("wl-copy").is_ok(),
            xdotool: which::which("xdotool").is_ok(),
        }
    }
}

// The session detection rules describe Linux and BSD; on Windows and macOS `detect_from`
// answers by `cfg!` and there is nothing to check
#[cfg(all(test, not(any(target_os = "windows", target_os = "macos"))))]
mod tests {
    use super::*;

    fn env(session: &str) -> SessionEnv {
        SessionEnv {
            session_type: Some(session.into()),
            ..SessionEnv::default()
        }
    }

    #[test]
    fn hyprland_is_recognised_by_its_instance_signature() {
        let mut e = env("wayland");
        e.hyprland_signature = Some("abc_123".into());
        assert_eq!(detect_from(&e), Platform::Wayland(Compositor::Hyprland));
    }

    #[test]
    fn sway_is_recognised_by_its_socket() {
        let mut e = env("wayland");
        e.swaysock = Some("/run/user/1000/sway-ipc.sock".into());
        assert_eq!(detect_from(&e), Platform::Wayland(Compositor::Sway));
    }

    #[test]
    fn kde_and_gnome_come_from_the_desktop_list() {
        let mut e = env("wayland");
        e.current_desktop = Some("KDE:wayland".into());
        assert_eq!(detect_from(&e), Platform::Wayland(Compositor::Kde));
        e.current_desktop = Some("ubuntu:GNOME".into());
        assert_eq!(detect_from(&e), Platform::Wayland(Compositor::Gnome));
    }

    #[test]
    fn unknown_wayland_compositor_is_still_wayland() {
        let mut e = env("wayland");
        e.current_desktop = Some("river".into());
        assert_eq!(detect_from(&e), Platform::Wayland(Compositor::Other));
    }

    #[test]
    fn x11_session_is_detected_by_type_or_display() {
        assert_eq!(detect_from(&env("x11")), Platform::X11);
        let e = SessionEnv {
            x11_display: Some(":0".into()),
            ..SessionEnv::default()
        };
        assert_eq!(detect_from(&e), Platform::X11);
    }

    #[test]
    fn wayland_display_alone_is_enough() {
        let e = SessionEnv {
            wayland_display: Some("wayland-1".into()),
            ..SessionEnv::default()
        };
        assert!(detect_from(&e).is_wayland());
    }

    #[test]
    fn a_bare_tty_is_headless_not_a_guess() {
        assert_eq!(detect_from(&SessionEnv::default()), Platform::Headless);
        assert_eq!(detect_from(&env("tty")), Platform::Headless);
    }

    #[test]
    fn labels_are_human_readable() {
        assert_eq!(
            Platform::Wayland(Compositor::Hyprland).label(),
            "Wayland/Hyprland"
        );
        assert_eq!(Platform::X11.label(), "X11");
        assert_eq!(
            Platform::Wayland(Compositor::Hyprland).compositor(),
            Some(Compositor::Hyprland)
        );
        assert_eq!(Platform::X11.compositor(), None);
    }
}
