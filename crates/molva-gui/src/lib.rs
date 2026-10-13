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
pub mod sidecar;
pub mod stats;
pub mod tray;

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

/// Error notice with a next step: the same in the system toast and in the window
pub fn notify_error(app: &AppHandle, message: &str, hint: Option<&str>) {
    use tauri_plugin_notification::NotificationExt;
    let body = match hint {
        Some(hint) => format!("{message}\n{}: {hint}", i18n::t("common.nextStep")),
        None => message.to_string(),
    };
    if let Err(err) = app
        .notification()
        .builder()
        .title("MolvAI")
        .body(&body)
        .show()
    {
        tracing::warn!(%err, "notification not shown");
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
        Event::State { state, mode } => {
            app.state::<AppState>().remember_state(Some(state));
            let _ = app.emit(
                "molva://state",
                serde_json::json!({ "state": state, "mode": mode }),
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
            let _ = app.emit("molva://entry", entry);
        }
        Event::Error {
            code,
            message,
            hint,
        } => {
            tracing::warn!(?code, %message, "daemon error");
            let message = format!("{}: {message}", i18n::daemon_error_title(code));
            notify_error(app, &message, hint.as_deref());
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
                    // We ask for the device list only when the daemon appears
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
            commands::list_devices,
            commands::inject_text,
            commands::reload_config,
            commands::start_daemon,
            commands::stop_daemon,
            commands::get_config,
            commands::get_config_path,
            commands::is_first_run,
            commands::save_config,
            commands::export_config,
            commands::import_config,
            commands::reset_config,
            commands::hyprland_snippet,
            commands::set_autostart,
            commands::get_autostart,
            commands::toggle_hotkeys_paused,
            commands::history_list,
            commands::history_apps,
            commands::history_delete,
            commands::history_clear,
            commands::data_dir_path,
            commands::open_path,
            commands::stats_summary,
            commands::stats_export_csv,
            commands::transcribe_file,
            commands::transcribe_cancel,
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
