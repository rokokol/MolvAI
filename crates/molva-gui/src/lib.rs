// SPDX-License-Identifier: MIT
//! Сборка приложения Tauri: состояние, плагины, трей, подписка на события демона.
//!
//! GUI — обычный клиент демона: микрофоном и моделью он не владеет и без демона
//! честно показывает, что демон не запущен, вместо пустого окна.

// В тестах паника — это способ сообщить о провале, а не необработанная ошибка.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod commands;
pub mod history;
pub mod hotkeys;
pub mod ipc;
pub mod sidecar;
pub mod stats;
pub mod tray;

use std::time::Duration;

use molva_core::ipc::{Command, Event};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager, WindowEvent};

use crate::commands::AppState;
use crate::ipc::Message;

/// Пауза между попытками переподключиться к демону.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Событие «демон появился/пропал» для бейджа состояния на Dashboard.
#[derive(Debug, Clone, Serialize)]
struct DaemonPresence {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

/// Уведомление об ошибке со следующим шагом: одно и то же и в системном тосте, и в окне.
pub fn notify_error(app: &AppHandle, message: &str, hint: Option<&str>) {
    use tauri_plugin_notification::NotificationExt;
    let body = match hint {
        Some(hint) => format!("{message}\nЧто делать: {hint}"),
        None => message.to_string(),
    };
    if let Err(err) = app
        .notification()
        .builder()
        .title("MolvAI")
        .body(&body)
        .show()
    {
        tracing::warn!(%err, "уведомление не показано");
    }
    let _ = app.emit(
        "molva://error",
        serde_json::json!({ "message": message, "hint": hint }),
    );
}

/// Пересобрать трей на главном потоке: часть оконных тулкитов не терпит иного.
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
            tracing::warn!(?code, %message, "ошибка демона");
            notify_error(app, &message, hint.as_deref());
        }
        Event::ConfigReloaded => {
            let _ = app.emit("molva://config", ());
            refresh_tray(app);
        }
        Event::DevicesChanged => {
            let state = app.state::<AppState>();
            if let Err(err) = commands::fetch_devices(&state) {
                tracing::info!(message = %err.message, "список устройств не обновлён");
            }
            let _ = app.emit("molva://devices", ());
            refresh_tray(app);
        }
    }
}

/// Держит подписку на события демона, переподключаясь, пока приложение живо.
fn spawn_subscription(app: AppHandle) {
    std::thread::spawn(move || loop {
        match ipc::Connection::connect() {
            Err(err) => {
                let _ = app.emit(
                    "molva://daemon",
                    DaemonPresence {
                        connected: false,
                        message: Some(err.to_string()),
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
                    // Список устройств спрашиваем только при появлении демона.
                    let _ = commands::fetch_devices(&app.state::<AppState>());
                    refresh_tray(&app);
                    loop {
                        match connection.recv() {
                            Ok(Some(Message::Event(event))) => handle_event(&app, event),
                            Ok(Some(Message::Response(_))) => continue,
                            Ok(None) => break,
                            Err(err) => {
                                tracing::info!(%err, "подписка прервана");
                                break;
                            }
                        }
                    }
                }
                let _ = app.emit(
                    "molva://daemon",
                    DaemonPresence {
                        connected: false,
                        message: Some("демон закрыл соединение".into()),
                        hint: Some("Проверьте, работает ли `molva daemon`".into()),
                    },
                );
                app.state::<AppState>().remember_state(None);
                refresh_tray(&app);
            }
        }
        std::thread::sleep(RECONNECT_DELAY);
    });
}

/// Запуск приложения. Ошибка чтения настроек не молчаливая: она видна в терминале.
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
            eprintln!("не удалось прочитать настройки: {err}");
            std::process::exit(6);
        }
    };

    let mut builder = tauri::Builder::default();

    // Плагин одиночного запуска регистрируется первым: вторая копия должна успеть
    // передать управление первой до создания собственного окна.
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
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // Значок в трее не должен уносить с собой окно: там, где нет
            // libappindicator, зависимость паникует, а приложение обязано открыться.
            let tray =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tray::init(&handle)));
            match tray {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!(%err, "значок в трее не создан"),
                Err(_) => tracing::warn!(
                    "значок в трее недоступен: не загрузилась libayatana-appindicator3"
                ),
            }
            spawn_subscription(handle.clone());

            #[cfg(not(target_os = "linux"))]
            hotkeys::register(&handle, &handle.state::<AppState>().config().hotkeys);

            // Фронтенд сообщает о готовности: до этого события `emit` некому слушать.
            let ready = handle.clone();
            app.listen("molva://ready", move |_| {
                if ready.state::<AppState>().first_run() {
                    let _ = ready.emit("molva://navigate", "settings");
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна прячет его в трей: демон продолжает работать.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение Tauri");
}
