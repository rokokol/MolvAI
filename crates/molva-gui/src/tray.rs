// SPDX-License-Identifier: MIT
//! Tray icon: daemon state and quick actions
//!
//! The menu is rebuilt in full at every change (state, style, devices). So the radio
//! button marks always match the settings and never drift from them.
//! There is no recording overlay on purpose: `gtk-layer-shell` is under the GPL,
//! so the tray shows the state

use molva_core::ipc::{Command, DaemonState};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::commands::{builtin_style_options, AppState};
use crate::{i18n, ipc};

pub const TRAY_ID: &str = "molva";

const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
const ICON_RECORDING: &[u8] = include_bytes!("../icons/tray-recording.png");
const ICON_PROCESSING: &[u8] = include_bytes!("../icons/tray-processing.png");

/// Output modes in the Output submenu; the label of each is `settings.output.<mode>`
const OUTPUT_MODES: [&str; 4] = ["auto", "paste", "type", "clipboard"];

/// Icon and hint for the daemon state. `None` means the daemon is not running
fn icon_bytes(state: Option<DaemonState>) -> &'static [u8] {
    match state {
        Some(DaemonState::Recording) => ICON_RECORDING,
        Some(DaemonState::Transcribing | DaemonState::PostProcessing | DaemonState::Injecting) => {
            ICON_PROCESSING
        }
        _ => ICON_IDLE,
    }
}

/// Which of the three images matches the state: idle, recording, processing
///
/// A small value, not bytes: it is cheap to store and to compare with the icon that the tray
/// already shows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIcon {
    Idle,
    Recording,
    Processing,
}

/// Icon for the daemon state
pub fn tray_icon(state: Option<DaemonState>) -> TrayIcon {
    match state {
        Some(DaemonState::Recording) => TrayIcon::Recording,
        Some(DaemonState::Transcribing | DaemonState::PostProcessing | DaemonState::Injecting) => {
            TrayIcon::Processing
        }
        _ => TrayIcon::Idle,
    }
}

/// Whether the tray icon needs a change
///
/// Criterion AM-11: **the tray icon is static by setting**. It changes only when the state
/// itself changes (idle, recording, processing, idle), not at every daemon event. The signal
/// level arrives ten times a second, and a reaction to each event would turn the tray into
/// a flasher. `gui.tray_animation = true` brings the flashing back for users who need it
/// more than calm
pub fn should_update_icon(shown: Option<TrayIcon>, next: TrayIcon, tray_animation: bool) -> bool {
    if tray_animation && next == TrayIcon::Recording {
        // Recording animation: the frames change during recording, and only by explicit setting
        return true;
    }
    shown != Some(next)
}

/// The dictionary key that names a daemon state, the same one the window's badge uses
fn state_key(state: Option<DaemonState>) -> &'static str {
    match state {
        None => "daemon.stopped",
        Some(DaemonState::Idle) => "state.idle",
        Some(DaemonState::Recording) => "state.recording",
        Some(DaemonState::Transcribing) => "state.transcribing",
        Some(DaemonState::PostProcessing) => "state.post_processing",
        Some(DaemonState::Injecting) => "state.injecting",
    }
}

/// The tray tooltip: a short phrase about what happens right now
pub fn tooltip(state: Option<DaemonState>) -> String {
    format!("MolvAI: {}", i18n::t(state_key(state)))
}

/// The label of the first menu item: it stops a recording in progress and starts one otherwise
fn record_label(state: Option<DaemonState>) -> &'static str {
    match state {
        Some(DaemonState::Recording) => "tray.recordStop",
        _ => "tray.recordStart",
    }
}

/// A menu item that opens a window tab, labelled as that tab in the window's navigation
fn open_tab_label(tab: &str) -> String {
    format!("{}…", i18n::t(&format!("nav.{tab}")))
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let config = state.config();
    let daemon_state = state.last_state();

    let record = MenuItem::with_id(
        app,
        "record",
        i18n::t(record_label(daemon_state)),
        true,
        None::<&str>,
    )?;
    // The second item is the panel itself: on Wayland it is the only way to a hidden window
    // besides the left click, and not every tray host passes the left click
    let open_panel = MenuItem::with_id(
        app,
        "open:dashboard",
        i18n::t("tray.openPanel"),
        true,
        None::<&str>,
    )?;

    let style_items: Vec<CheckMenuItem<Wry>> = builtin_style_options(&config)
        .into_iter()
        .map(|(id, name)| {
            CheckMenuItem::with_id(
                app,
                format!("style:{id}"),
                name,
                true,
                id == config.style.default,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<_>>()?;
    let style_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = style_items
        .iter()
        .map(|i| i as &dyn tauri::menu::IsMenuItem<Wry>)
        .collect();
    let style_menu = Submenu::with_items(app, i18n::t("dashboard.style"), true, &style_refs)?;

    let output_items: Vec<CheckMenuItem<Wry>> = OUTPUT_MODES
        .iter()
        .map(|id| {
            CheckMenuItem::with_id(
                app,
                format!("output:{id}"),
                i18n::t(&format!("settings.output.{id}")),
                true,
                *id == config.output.mode,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<_>>()?;
    let output_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = output_items
        .iter()
        .map(|i| i as &dyn tauri::menu::IsMenuItem<Wry>)
        .collect();
    let output_menu =
        Submenu::with_items(app, i18n::t("settings.section.output"), true, &output_refs)?;

    let devices = state.devices();
    let mic_items: Vec<CheckMenuItem<Wry>> = if devices.is_empty() {
        vec![CheckMenuItem::with_id(
            app,
            "mic:none",
            i18n::t("tray.devicesUnavailable"),
            false,
            false,
            None::<&str>,
        )?]
    } else {
        std::iter::once(CheckMenuItem::with_id(
            app,
            "mic:default",
            i18n::t("settings.deviceDefault"),
            true,
            config.audio.device == "default",
            None::<&str>,
        ))
        .chain(devices.iter().map(|device| {
            CheckMenuItem::with_id(
                app,
                format!("mic:{}", device.name),
                &device.name,
                true,
                config.audio.device == device.name,
                None::<&str>,
            )
        }))
        .collect::<tauri::Result<_>>()?
    };
    let mic_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = mic_items
        .iter()
        .map(|i| i as &dyn tauri::menu::IsMenuItem<Wry>)
        .collect();
    let mic_menu = Submenu::with_items(app, i18n::t("tray.microphone"), true, &mic_refs)?;

    let open_tab = |tab: &str| {
        MenuItem::with_id(
            app,
            format!("open:{tab}"),
            open_tab_label(tab),
            true,
            None::<&str>,
        )
    };
    let history = open_tab("history")?;
    let settings = open_tab("settings")?;
    let stats_item = open_tab("stats")?;
    let pause = CheckMenuItem::with_id(
        app,
        "pause",
        i18n::t("tray.pauseHotkeys"),
        true,
        state.hotkeys_paused(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", i18n::t("tray.quit"), true, None::<&str>)?;

    Menu::with_items(
        app,
        &[
            &record,
            &open_panel,
            &PredefinedMenuItem::separator(app)?,
            &style_menu,
            &output_menu,
            &mic_menu,
            &PredefinedMenuItem::separator(app)?,
            &history,
            &settings,
            &stats_item,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &quit,
        ],
    )
}

/// The icon that the tray shows now: 0 means none yet, other values give the image number
static SHOWN_ICON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn shown_icon() -> Option<TrayIcon> {
    match SHOWN_ICON.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(TrayIcon::Idle),
        2 => Some(TrayIcon::Recording),
        3 => Some(TrayIcon::Processing),
        _ => None,
    }
}

fn remember_icon(icon: TrayIcon) {
    let code = match icon {
        TrayIcon::Idle => 1,
        TrayIcon::Recording => 2,
        TrayIcon::Processing => 3,
    };
    SHOWN_ICON.store(code, std::sync::atomic::Ordering::Relaxed);
}

/// Create the tray icon when the application starts
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    remember_icon(TrayIcon::Idle);
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICON_IDLE)?)
        .tooltip(tooltip(None))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            // A left click on the icon shows the window. It is the only way to bring back
            // a hidden window on Wayland, where the application cannot raise itself
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle(), None);
            }
        })
        .build(app)?;
    Ok(())
}

/// Update the icon, the hint and the menu for the current state
pub fn refresh(app: &AppHandle) {
    let app_state = app.state::<AppState>();
    let state = app_state.last_state();
    let tray_animation = app_state.config().gui.tray_animation;
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let next = tray_icon(state);
    if should_update_icon(shown_icon(), next, tray_animation) {
        if let Ok(image) = Image::from_bytes(icon_bytes(state)) {
            let _ = tray.set_icon(Some(image));
            remember_icon(next);
        }
    }
    let _ = tray.set_tooltip(Some(tooltip(state)));
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => tracing::warn!(%err, "tray menu not rebuilt"),
    }
}

/// Show the window and, if needed, switch it to a tab
pub fn show_window(app: &AppHandle, tab: Option<&str>) {
    use tauri::Emitter;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    if let Some(tab) = tab {
        let _ = app.emit("molva://navigate", tab);
    }
}

/// Window tabs that the tray can open. The list is closed: an arbitrary `open:*`
/// identifier must not send the window to a tab that does not exist
const WINDOW_TABS: [&str; 4] = ["dashboard", "history", "stats", "settings"];

/// Menu item `open:<tab>` to the tab name for the window, `None` for all other items
fn window_tab_for_menu_id(menu_id: &str) -> Option<&'static str> {
    let tab = menu_id.strip_prefix("open:")?;
    WINDOW_TABS.iter().copied().find(|known| *known == tab)
}

/// Send a command to the daemon in a separate thread: a hung daemon must not freeze the menu
fn send_async(app: &AppHandle, command: Command) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = ipc::request(command) {
            crate::notify_error(&app, &err.user_message(), err.hint().as_deref());
        }
    });
}

// `on_menu_event` sets the signature: the event arrives by value
#[allow(clippy::needless_pass_by_value)]
fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().0.clone();
    match id.as_str() {
        "record" => {
            let recording = matches!(
                app.state::<AppState>().last_state(),
                Some(DaemonState::Recording)
            );
            let command = if recording {
                Command::RecordStop
            } else {
                Command::RecordStart {
                    mode: molva_core::domain::entry::Mode::Dictation,
                    style: None,
                }
            };
            send_async(app, command);
        }
        "pause" => {
            let state = app.state::<AppState>();
            let paused = !state.hotkeys_paused();
            state.set_hotkeys_paused(paused);
            #[cfg(not(target_os = "linux"))]
            if paused {
                crate::hotkeys::unregister(app);
            } else {
                crate::hotkeys::register(app, &state.config().hotkeys);
            }
            refresh(app);
        }
        "quit" => {
            app.state::<AppState>().stop_owned_daemon();
            app.exit(0);
        }
        other => {
            if let Some(tab) = window_tab_for_menu_id(other) {
                show_window(app, Some(tab));
            } else if let Some(style) = other.strip_prefix("style:") {
                set_style(app, style);
            } else if let Some(mode) = other.strip_prefix("output:") {
                set_output(app, mode);
            } else if let Some(device) = other.strip_prefix("mic:") {
                set_device(app, device);
            }
        }
    }
}

/// Changes from the tray go to the config: otherwise the choice is lost after a restart
fn update_config(app: &AppHandle, change: impl FnOnce(&mut molva_core::Config)) {
    let state = app.state::<AppState>();
    let mut config = state.config();
    change(&mut config);
    if let Err(err) = state.replace_config(config) {
        crate::notify_error(app, &err.to_string(), None);
        return;
    }
    send_async(app, Command::ConfigReload);
    refresh(app);
}

pub(crate) fn set_style(app: &AppHandle, style: &str) {
    let style = style.to_string();
    update_config(app, |config| config.style.default.clone_from(&style));
    send_async(app, Command::StyleSet { style });
}

fn set_output(app: &AppHandle, mode: &str) {
    let mode = mode.to_string();
    update_config(app, |config| config.output.mode = mode);
}

fn set_device(app: &AppHandle, device: &str) {
    let device = device.to_string();
    update_config(app, |config| config.audio.device = device);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_STATES: [Option<DaemonState>; 6] = [
        None,
        Some(DaemonState::Idle),
        Some(DaemonState::Recording),
        Some(DaemonState::Transcribing),
        Some(DaemonState::PostProcessing),
        Some(DaemonState::Injecting),
    ];

    #[test]
    fn every_state_has_its_own_wording_in_both_languages() {
        for lang in [i18n::Lang::En, i18n::Lang::Ru] {
            let wordings: std::collections::BTreeSet<String> = ALL_STATES
                .iter()
                .map(|state| i18n::translate(lang, state_key(*state), &[]))
                .collect();
            assert_eq!(wordings.len(), ALL_STATES.len(), "{lang:?}: {wordings:?}");
            assert!(
                wordings.iter().all(|text| !text.contains('.')),
                "{lang:?}: a state shows its key: {wordings:?}"
            );
        }
    }

    #[test]
    fn menu_labels_are_in_the_dictionaries() {
        for state in ALL_STATES {
            let key = record_label(state);
            assert_ne!(i18n::translate(i18n::Lang::Ru, key, &[]), key);
        }
        for tab in WINDOW_TABS {
            let key = format!("nav.{tab}");
            assert_ne!(i18n::translate(i18n::Lang::Ru, &key, &[]), key);
        }
        for mode in OUTPUT_MODES {
            let key = format!("settings.output.{mode}");
            assert_ne!(i18n::translate(i18n::Lang::Ru, &key, &[]), key);
        }
    }

    #[test]
    fn recording_and_processing_have_different_icons() {
        assert_ne!(
            icon_bytes(Some(DaemonState::Recording)),
            icon_bytes(Some(DaemonState::Transcribing))
        );
        assert_eq!(icon_bytes(None), icon_bytes(Some(DaemonState::Idle)));
    }

    #[test]
    fn processing_stages_share_one_icon() {
        assert_eq!(
            icon_bytes(Some(DaemonState::PostProcessing)),
            icon_bytes(Some(DaemonState::Injecting))
        );
    }

    #[test]
    fn record_item_offers_the_opposite_of_the_current_state() {
        assert_eq!(
            record_label(Some(DaemonState::Recording)),
            "tray.recordStop"
        );
        assert_eq!(record_label(None), "tray.recordStart");
        assert_eq!(record_label(Some(DaemonState::Idle)), "tray.recordStart");
    }

    /// How many times the tray changes the icon for the listed events
    fn icon_updates(events: &[Option<DaemonState>], tray_animation: bool) -> usize {
        let mut shown = Some(TrayIcon::Idle);
        let mut updates = 0;
        for state in events {
            let next = tray_icon(*state);
            if should_update_icon(shown, next, tray_animation) {
                updates += 1;
                shown = Some(next);
            }
        }
        updates
    }

    #[test]
    fn the_tray_icon_changes_at_most_twice_during_a_reply() {
        // Criterion AM-11: during a phrase the icon changes to "recording" and to
        // "processing", and no more. The signal level arrives in tens of events, but the state does not change
        let mut reply = vec![Some(DaemonState::Recording); 40];
        reply.extend([
            Some(DaemonState::Transcribing),
            Some(DaemonState::PostProcessing),
            Some(DaemonState::Injecting),
        ]);
        assert_eq!(
            icon_updates(&reply, false),
            2,
            "at most two icon updates per phrase"
        );
        // The return to idle is the third and last update of the cycle
        reply.push(Some(DaemonState::Idle));
        assert_eq!(icon_updates(&reply, false), 3);
    }

    #[test]
    fn repeated_events_with_the_same_state_do_not_touch_the_icon() {
        let idle = vec![Some(DaemonState::Idle); 100];
        assert_eq!(icon_updates(&idle, false), 0, "the icon must not flash");
    }

    #[test]
    fn the_setting_brings_the_recording_animation_back() {
        let recording = vec![Some(DaemonState::Recording); 10];
        assert_eq!(
            icon_updates(&recording, true),
            10,
            "with tray_animation = true the recording frames change"
        );
        assert!(!molva_core::Config::default().gui.tray_animation);
    }

    #[test]
    fn panel_item_opens_the_dashboard_tab() {
        assert_eq!(window_tab_for_menu_id("open:dashboard"), Some("dashboard"));
        assert_eq!(window_tab_for_menu_id("open:history"), Some("history"));
        assert_eq!(window_tab_for_menu_id("open:settings"), Some("settings"));
        assert_eq!(window_tab_for_menu_id("open:stats"), Some("stats"));
    }

    #[test]
    fn unknown_open_targets_and_other_items_do_not_open_the_window() {
        assert_eq!(window_tab_for_menu_id("open:nowhere"), None);
        assert_eq!(window_tab_for_menu_id("record"), None);
        assert_eq!(window_tab_for_menu_id("style:cleanup"), None);
    }

    #[test]
    fn tray_icons_are_valid_png_files() {
        for bytes in [ICON_IDLE, ICON_RECORDING, ICON_PROCESSING] {
            assert_eq!(&bytes[1..4], b"PNG", "the tray icon is not a PNG");
        }
    }
}
