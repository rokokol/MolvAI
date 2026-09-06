// SPDX-License-Identifier: MIT
//! Значок в трее: состояние демона и быстрые действия.
//!
//! Меню пересобирается целиком при каждом изменении (состояние, стиль, устройства):
//! так отметки радиокнопок всегда соответствуют настройкам, а не расходятся с ними.
//! Оверлея записи нет намеренно — `gtk-layer-shell` под GPL, поэтому состояние показывает трей.

use molva_core::ipc::{Command, DaemonState};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::commands::{builtin_style_options, AppState};
use crate::ipc;

pub const TRAY_ID: &str = "molva";

const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
const ICON_RECORDING: &[u8] = include_bytes!("../icons/tray-recording.png");
const ICON_PROCESSING: &[u8] = include_bytes!("../icons/tray-processing.png");

/// Режимы вывода в подменю Output.
const OUTPUT_MODES: [(&str, &str); 4] = [
    ("auto", "Авто"),
    ("paste", "Вставка"),
    ("type", "Набор"),
    ("clipboard", "Только буфер"),
];

/// Иконка и подсказка по состоянию демона. `None` — демон не запущен.
fn icon_bytes(state: Option<DaemonState>) -> &'static [u8] {
    match state {
        Some(DaemonState::Recording) => ICON_RECORDING,
        Some(DaemonState::Transcribing | DaemonState::PostProcessing | DaemonState::Injecting) => {
            ICON_PROCESSING
        }
        _ => ICON_IDLE,
    }
}

/// Текст подсказки трея: короткая фраза о том, что происходит прямо сейчас.
pub fn tooltip(state: Option<DaemonState>) -> &'static str {
    match state {
        None => "MolvAI: демон не запущен",
        Some(DaemonState::Idle) => "MolvAI: готов",
        Some(DaemonState::Recording) => "MolvAI: идёт запись",
        Some(DaemonState::Transcribing) => "MolvAI: распознавание",
        Some(DaemonState::PostProcessing) => "MolvAI: обработка текста",
        Some(DaemonState::Injecting) => "MolvAI: вставка текста",
    }
}

/// Надпись на первом пункте меню: во время записи он останавливает, иначе начинает.
fn record_label(state: Option<DaemonState>) -> &'static str {
    match state {
        Some(DaemonState::Recording) => "Остановить запись",
        _ => "Начать запись",
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let config = state.config();
    let daemon_state = state.last_state();

    let record = MenuItem::with_id(
        app,
        "record",
        record_label(daemon_state),
        true,
        None::<&str>,
    )?;
    // Второй пункт — сама панель: на Wayland это единственный путь к спрятанному окну
    // помимо левого щелчка, а его не все хосты трея передают.
    let open_panel =
        MenuItem::with_id(app, "open:dashboard", "Открыть панель…", true, None::<&str>)?;

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
    let style_menu = Submenu::with_items(app, "Стиль", true, &style_refs)?;

    let output_items: Vec<CheckMenuItem<Wry>> = OUTPUT_MODES
        .iter()
        .map(|(id, name)| {
            CheckMenuItem::with_id(
                app,
                format!("output:{id}"),
                *name,
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
    let output_menu = Submenu::with_items(app, "Вывод", true, &output_refs)?;

    let devices = state.devices();
    let mic_items: Vec<CheckMenuItem<Wry>> = if devices.is_empty() {
        vec![CheckMenuItem::with_id(
            app,
            "mic:none",
            "Список устройств недоступен",
            false,
            false,
            None::<&str>,
        )?]
    } else {
        std::iter::once(CheckMenuItem::with_id(
            app,
            "mic:default",
            "Устройство по умолчанию",
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
    let mic_menu = Submenu::with_items(app, "Микрофон", true, &mic_refs)?;

    let history = MenuItem::with_id(app, "open:history", "История…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "open:settings", "Настройки…", true, None::<&str>)?;
    let stats_item = MenuItem::with_id(app, "open:stats", "Статистика…", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(
        app,
        "pause",
        "Приостановить хоткеи",
        true,
        state.hotkeys_paused(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;

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

/// Создать значок в трее при запуске приложения.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICON_IDLE)?)
        .tooltip(tooltip(None))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            // Левый щелчок по значку показывает окно: это единственный способ вернуть
            // спрятанное окно на Wayland, где приложение не может поднять себя иначе.
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

/// Обновить значок, подсказку и меню под текущее состояние.
pub fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>().last_state();
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Ok(image) = Image::from_bytes(icon_bytes(state)) {
        let _ = tray.set_icon(Some(image));
    }
    let _ = tray.set_tooltip(Some(tooltip(state)));
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => tracing::warn!(%err, "меню трея не пересобрано"),
    }
}

/// Показать окно и, если нужно, переключить его на вкладку.
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

/// Вкладки окна, которые открываются из трея. Список закрыт: произвольный
/// идентификатор `open:*` не должен уводить окно на несуществующую вкладку.
const WINDOW_TABS: [&str; 4] = ["dashboard", "history", "stats", "settings"];

/// Пункт меню `open:<вкладка>` → имя вкладки для окна, `None` для всех прочих пунктов.
fn window_tab_for_menu_id(menu_id: &str) -> Option<&'static str> {
    let tab = menu_id.strip_prefix("open:")?;
    WINDOW_TABS.iter().copied().find(|known| *known == tab)
}

/// Команда демону в отдельном потоке: зависший демон не должен морозить меню.
fn send_async(app: &AppHandle, command: Command) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = ipc::request(command) {
            crate::notify_error(&app, &err.to_string(), err.hint().as_deref());
        }
    });
}

// Сигнатуру задаёт `on_menu_event`: событие приходит по значению.
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

/// Изменения из трея пишутся в конфиг: иначе после перезапуска выбор пропадёт.
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

fn set_style(app: &AppHandle, style: &str) {
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

    #[test]
    fn every_state_has_its_own_wording() {
        assert!(tooltip(None).contains("не запущен"));
        assert!(tooltip(Some(DaemonState::Recording)).contains("запись"));
        assert!(tooltip(Some(DaemonState::Transcribing)).contains("распознавание"));
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
            "Остановить запись"
        );
        assert_eq!(record_label(None), "Начать запись");
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
            assert_eq!(&bytes[1..4], b"PNG", "иконка трея не PNG");
        }
    }
}
