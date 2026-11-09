// SPDX-License-Identifier: MIT
//! Assembly of the Tauri application: state, plugins, tray, subscription to daemon events
//!
//! The GUI is a plain client of the daemon. It owns neither the microphone nor the model.
//! Without a daemon it states plainly that the daemon is not running, instead of showing
//! an empty window

// In tests a panic is a way to report a failure, not an unhandled error. An exact
// comparison of floating-point numbers is legitimate: the test compares a computed value
// with a constant that the test itself set. Printing measurements to stdout is the report
// of a benchmark test
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::print_stdout
    )
)]

pub mod commands;
pub mod history;
pub mod hotkeys;
pub mod i18n;
pub mod ipc;
pub mod media_server;
pub mod paths;
pub mod providers;
pub mod sidecar;
pub mod stats;
pub mod tray;

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use molva_core::ipc::{Command, Event};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager, WindowEvent};

use crate::commands::AppState;
use crate::ipc::Message;

/// Mutex lock that survives a panic in another thread
///
/// The GUI locks guard the settings, the device cache and the handle of the running daemon.
/// A panic elsewhere does not make these values inconsistent. To crash the whole
/// application because of a poisoned lock would be worse than to continue with the same
/// contents
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pause between attempts to reconnect to the daemon
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Event "daemon appeared or disappeared" for the state badge on the Dashboard
#[derive(Debug, Clone, Serialize)]
struct DaemonPresence {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

/// Whether the main window is hidden in the tray
fn window_hidden(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return true;
    };
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    !visible || minimized
}

/// A system toast for work that happens while the window is hidden
/// The app icon as a file for the notification server: the plugin takes a path, not bytes
fn notification_icon() -> Option<PathBuf> {
    // A fresh path and a fresh write: a notification server caches an icon by its path, and a
    // stale temp file from an older build would keep the old blue mark
    let path = std::env::temp_dir().join("molvai-notification-icon-pink.png");
    std::fs::write(&path, include_bytes!("../icons/notification.png")).ok()?;
    Some(path)
}

fn system_notification(app: &AppHandle, message: &str, hint: Option<&str>) {
    use tauri_plugin_notification::NotificationExt;
    let body = match hint {
        Some(hint) => format!("{message}\n{}: {hint}", i18n::t("common.nextStep")),
        None => message.to_string(),
    };
    let mut builder = app.notification().builder().title("MolvAI").body(&body);
    if let Some(icon) = notification_icon() {
        builder = builder.icon(icon.to_string_lossy());
    }
    if let Err(err) = builder.show() {
        tracing::warn!(%err, "notification not shown");
    }
}

/// A hidden window announces the style that the person just switched to
pub(crate) fn notify_style(app: &AppHandle, name: &str) {
    if window_hidden(app) {
        system_notification(app, &i18n::tv("style.changed", &[("name", name)]), None);
    }
}

/// Error notice with a next step: the window always gets it; the toast only when hidden
pub fn notify_error(app: &AppHandle, message: &str, hint: Option<&str>) {
    if window_hidden(app) {
        system_notification(app, message, hint);
    }
    let _ = app.emit(
        "molva://error",
        serde_json::json!({ "message": message, "hint": hint }),
    );
}

/// Rebuild the tray on the main thread: some window toolkits accept nothing else
fn refresh_tray(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || tray::refresh(&handle));
}

fn handle_event(app: &AppHandle, event: Event) {
    match event {
        // The state event carries the state machine and the job of the worker: the window
        // shows "recording" and "transcribing a file" from the same message, so the two can
        // never disagree
        Event::State { state, mode, job } => {
            app.state::<AppState>().remember_state(Some(state));
            let _ = app.emit(
                "molva://state",
                serde_json::json!({ "state": state, "mode": mode, "job": job }),
            );
            refresh_tray(app);
        }
        Event::Level { rms } => {
            let _ = app.emit("molva://level", rms);
        }
        Event::Hypothesis { text } => {
            let _ = app.emit("molva://hypothesis", text);
        }
        Event::Entry { entry } => {
            // A phrase that reached the field with a failed model or paste is not silent: the
            // entry carries the reason, and the window must show it
            if let Some(error) = entry.error.as_deref() {
                notify_error(app, error, None);
            }
            let _ = app.emit("molva://entry", entry);
        }
        Event::Error {
            code,
            message,
            hint,
        } => {
            tracing::warn!(?code, %message, "daemon error");
            let title = i18n::daemon_error_title(code);
            // The window gets the detailed daemon text. A hidden window gets one short toast
            // in the interface language, without the English daemon detail
            let detailed = format!("{title}: {message}");
            if window_hidden(app) {
                system_notification(app, &title, None);
            }
            let _ = app.emit(
                "molva://error",
                serde_json::json!({ "message": detailed, "hint": hint }),
            );
        }
        Event::StyleChanged { style } => {
            let state = app.state::<AppState>();
            if state.managed() {
                state.remember_style(&style);
            } else {
                let mut config = state.config();
                config.style.default.clone_from(&style);
                if let Err(err) = state.replace_config(config) {
                    tracing::warn!(%err, "the style was not written to the settings file");
                }
            }
            let name = commands::builtin_style_options(&state.config())
                .into_iter()
                .find(|(id, _)| id == &style)
                .map_or(style, |(_, name)| name);
            notify_style(app, &name);
            let _ = app.emit("molva://config", ());
            refresh_tray(app);
        }
        Event::ConfigReloaded => {
            let _ = app.emit("molva://config", ());
            refresh_tray(app);
        }
        Event::DevicesChanged => {
            let state = app.state::<AppState>();
            if let Err(err) = commands::fetch_devices(&state) {
                tracing::info!(message = %err.message, "device list not refreshed");
            }
            let _ = app.emit("molva://devices", ());
            refresh_tray(app);
        }
    }
}

/// Keeps the subscription to daemon events and reconnects while the application lives
fn spawn_subscription(app: AppHandle) {
    std::thread::spawn(move || loop {
        match ipc::Connection::connect() {
            Err(err) => {
                let _ = app.emit(
                    "molva://daemon",
                    DaemonPresence {
                        connected: false,
                        message: Some(err.user_message()),
                        hint: err.hint(),
                    },
                );
                app.state::<AppState>().remember_state(None);
                refresh_tray(&app);
            }
            Ok(mut connection) => {
                if connection.send(Command::Subscribe { levels: true }).is_ok() {
                    let _ = app.emit(
                        "molva://daemon",
                        DaemonPresence {
                            connected: true,
                            message: None,
                            hint: None,
                        },
                    );
                    // The device list is asked for only when the daemon appears
                    let _ = commands::fetch_devices(&app.state::<AppState>());
                    refresh_tray(&app);
                    loop {
                        match connection.recv() {
                            Ok(Some(Message::Event(event))) => handle_event(&app, event),
                            Ok(Some(Message::Response(_))) => {}
                            Ok(None) => break,
                            Err(err) => {
                                tracing::info!(%err, "subscription interrupted");
                                break;
                            }
                        }
                    }
                }
                let _ = app.emit(
                    "molva://daemon",
                    DaemonPresence {
                        connected: false,
                        message: Some(i18n::t("daemon.closed")),
                        hint: Some(i18n::t("daemon.closedHint")),
                    },
                );
                app.state::<AppState>().remember_state(None);
                refresh_tray(&app);
            }
        }
        std::thread::sleep(RECONNECT_DELAY);
    });
}

/// Start the application. An error while reading the settings is not silent: it shows in
/// the terminal
// The Tauri build is the only failure with nobody to return it to: see `expect` at the end
// of the function
#[allow(clippy::expect_used)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MOLVA_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let state = match AppState::load() {
        Ok(state) => state,
        Err(err) => {
            eprintln!("cannot read the settings: {err}");
            std::process::exit(6);
        }
    };

    let mut builder = tauri::Builder::default();

    // The single-instance plugin registers first: a second copy must hand control
    // to the first one before it creates its own window
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_window(app, None);
        }));
    }

    builder = builder
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ));

    #[cfg(not(target_os = "linux"))]
    {
        builder = builder.plugin(hotkeys::plugin());
    }

    builder
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::record_start,
            commands::record_stop,
            commands::record_toggle,
            commands::record_cancel,
            commands::set_style,
            commands::available_styles,
            commands::builtin_styles,
            commands::stt_models,
            commands::stt_remote_available,
            commands::test_stt,
            commands::test_llm,
            commands::llm_providers,
            commands::provider_key_status,
            commands::store_provider_key,
            commands::remove_provider_key,
            commands::check_hotkey,
            commands::list_devices,
            commands::inject_text,
            commands::reload_config,
            commands::start_daemon,
            commands::stop_daemon,
            commands::get_config,
            commands::config_managed,
            commands::get_config_path,
            commands::is_first_run,
            commands::save_config,
            commands::export_config,
            commands::import_config,
            commands::reset_config,
            commands::hyprland_binds,
            commands::write_hyprland_binds,
            commands::set_autostart,
            commands::get_autostart,
            commands::toggle_hotkeys_paused,
            commands::history_list,
            commands::history_apps,
            commands::search_files,
            commands::history_delete,
            commands::history_clear,
            commands::data_dir_path,
            commands::open_path,
            commands::stats_summary,
            commands::stats_export_csv,
            commands::transcribe_file,
            commands::prepare_media,
            commands::path_suggestions,
            commands::ui_language,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // The tray icon must not take the window down with it: where libappindicator
            // is missing, the dependency panics, but the application must still open
            let tray =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tray::init(&handle)));
            match tray {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!(%err, "tray icon not created"),
                Err(_) => tracing::warn!(
                    "tray icon unavailable: libayatana-appindicator3 did not load"
                ),
            }
            spawn_subscription(handle.clone());

            #[cfg(not(target_os = "linux"))]
            hotkeys::register(&handle, &handle.state::<AppState>().config().hotkeys);

            // The frontend reports that it is ready: before this event, nobody listens to `emit`
            let ready = handle.clone();
            app.listen("molva://ready", move |_| {
                if ready.state::<AppState>().first_run() {
                    let _ = ready.emit("molva://navigate", "settings");
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it in the tray: the daemon keeps running
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        // The only place with nobody to return an error to: the application did not build,
        // there is no window, and nothing can show a message. The panic here is the report
        // to the user
        .expect("cannot start the Tauri application");
}
