// SPDX-License-Identifier: MIT
//! Команды, доступные фронтенду, и общее состояние приложения.
//!
//! Всё, что можно посчитать в Rust, считается здесь: фронтенд получает готовые структуры,
//! а не повторяет логику ядра на TypeScript.

// Сигнатуру команды задаёт Tauri: `State` и разобранные из JSON аргументы приходят по
// значению, ссылку `invoke_handler` принять не умеет.
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use chrono::Utc;
use molva_core::config::ConfigError;
use molva_core::domain::entry::Mode;
use molva_core::domain::{DeviceInfo, Entry, OutputMode};
use molva_core::ipc::{Command, DaemonState};
use molva_core::Config;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

use crate::history::{self, Filter, HistoryError};
use crate::ipc::{self, IpcClientError};
use crate::lock;
use crate::sidecar::{self, Daemon, SidecarError, Transcriptions};
use crate::stats::{self, StatsSummary};

/// Ошибка команды в том виде, в котором её показывает интерфейс: причина, подсказка, поле.
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    /// `daemon_unavailable` | `daemon` | `config` | `validation` | `history` | `sidecar`.
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Поле формы настроек, к которому относится ошибка валидации.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl CommandError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            hint: None,
            field: None,
        }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: Option<String>) -> Self {
        self.hint = hint;
        self
    }
}

impl From<IpcClientError> for CommandError {
    fn from(err: IpcClientError) -> Self {
        let kind = if err.is_unavailable() {
            "daemon_unavailable"
        } else {
            "daemon"
        };
        let hint = err.hint();
        Self::new(kind, err.to_string()).with_hint(hint)
    }
}

impl From<ConfigError> for CommandError {
    fn from(err: ConfigError) -> Self {
        Self::new("config", err.to_string())
    }
}

impl From<HistoryError> for CommandError {
    fn from(err: HistoryError) -> Self {
        Self::new("history", err.to_string())
    }
}

impl From<SidecarError> for CommandError {
    fn from(err: SidecarError) -> Self {
        let hint = match err {
            SidecarError::NotFound | SidecarError::Spawn { .. } => Some(
                "Соберите CLI (`cargo build -p molva`) и положите `molva` рядом с GUI или в PATH"
                    .to_string(),
            ),
            _ => None,
        };
        Self::new("sidecar", err.to_string()).with_hint(hint)
    }
}

/// Ошибка валидации настроек: поле, причина, допустимые значения.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
    pub allowed: Vec<String>,
}

impl From<ValidationError> for CommandError {
    fn from(err: ValidationError) -> Self {
        let hint = (!err.allowed.is_empty())
            .then(|| format!("Допустимые значения: {}", err.allowed.join(", ")));
        Self {
            kind: "validation".into(),
            message: err.message,
            hint,
            field: Some(err.field),
        }
    }
}

fn one_of(field: &str, value: &str, allowed: &[&str]) -> Result<(), ValidationError> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(ValidationError {
        field: field.into(),
        message: format!("недопустимое значение «{value}»"),
        allowed: allowed.iter().map(ToString::to_string).collect(),
    })
}

fn in_range(field: &str, value: f32, min: f32, max: f32) -> Result<(), ValidationError> {
    if value >= min && value <= max {
        return Ok(());
    }
    Err(ValidationError {
        field: field.into(),
        message: format!("значение {value} вне диапазона"),
        allowed: vec![format!("от {min} до {max}")],
    })
}

/// Проверка настроек перед записью на диск: пользователь видит поле и допустимые значения.
pub fn validate_config(config: &Config) -> Result<(), ValidationError> {
    one_of("ui_language", &config.ui_language, &["ru", "en"])?;
    one_of(
        "output.mode",
        &config.output.mode,
        &["auto", "paste", "type", "clipboard"],
    )?;
    one_of(
        "stt.engine",
        &config.stt.engine,
        &["whisper-cpp", "remote-openai"],
    )?;
    one_of(
        "hotkeys.backend",
        &config.hotkeys.backend,
        &["auto", "external", "evdev", "gui"],
    )?;
    one_of(
        "log.level",
        &config.log.level,
        &["error", "warn", "info", "debug", "trace"],
    )?;
    one_of(
        "llm.api_key_source",
        &config.llm.api_key_source,
        &["keyring", "env"],
    )?;
    in_range("audio.gain", config.audio.gain, 0.1, 10.0)?;
    in_range("audio.sound_volume", config.audio.sound_volume, 0.0, 1.0)?;
    in_range("llm.temperature", config.llm.temperature, 0.0, 2.0)?;
    if config.audio.max_duration_secs == 0 {
        return Err(ValidationError {
            field: "audio.max_duration_secs".into(),
            message: "длительность записи не может быть нулевой".into(),
            allowed: vec!["от 1 секунды".into()],
        });
    }
    if config.stt.model.trim().is_empty() {
        return Err(ValidationError {
            field: "stt.model".into(),
            message: "модель распознавания не выбрана".into(),
            allowed: vec![
                "tiny".into(),
                "base".into(),
                "small".into(),
                "medium".into(),
            ],
        });
    }
    if config.llm.enabled && config.llm.base_url.trim().is_empty() {
        return Err(ValidationError {
            field: "llm.base_url".into(),
            message: "у включённой модели должен быть адрес".into(),
            allowed: vec!["http://localhost:11434/v1".into()],
        });
    }
    Ok(())
}

/// Облачный ли провайдер: для него интерфейс требует явного подтверждения.
pub fn is_cloud_provider(provider: &str) -> bool {
    !matches!(provider, "ollama" | "lmstudio" | "custom")
}

/// Встроенные стили постобработки: идентификатор и подпись для меню.
// Идентификаторы совпадают с `molva_core::app::styles::builtin`: демон принимает только их,
// и чужой идентификатор молча откатывался бы на стиль по умолчанию.
pub const BUILTIN_STYLES: [(&str, &str); 6] = [
    ("verbatim", "Как сказано"),
    ("cleanup", "Чистка"),
    ("messenger", "Мессенджер"),
    ("mail", "Письмо"),
    ("code", "Код"),
    ("formal", "Формально"),
];

/// Встроенные стили плюс пользовательские из конфига, без повторов по идентификатору.
pub fn builtin_style_options(config: &Config) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = BUILTIN_STYLES
        .iter()
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect();
    for style in &config.style.custom {
        if options.iter().any(|(id, _)| id == &style.id) {
            continue;
        }
        options.push((style.id.clone(), style.name.clone()));
    }
    options
}

/// Стиль в виде, удобном для выпадающих списков интерфейса.
#[derive(Debug, Clone, Serialize)]
pub struct StyleOption {
    pub id: String,
    pub name: String,
}

#[tauri::command]
pub fn available_styles(state: State<'_, AppState>) -> Vec<StyleOption> {
    builtin_style_options(&state.config())
        .into_iter()
        .map(|(id, name)| StyleOption { id, name })
        .collect()
}

/// Общее состояние: настройки, запущенный нами демон, разборы файлов, пауза хоткеев.
#[derive(Debug)]
pub struct AppState {
    config: Mutex<Config>,
    config_path: PathBuf,
    /// Файла настроек не было — интерфейс открывается сразу на вкладке Settings.
    first_run: bool,
    daemon: Mutex<Daemon>,
    transcriptions: Transcriptions,
    hotkeys_paused: AtomicBool,
    last_state: Mutex<Option<DaemonState>>,
    /// Список устройств кэшируется: меню трея пересобирается часто, а опрос демона — нет.
    devices: Mutex<Vec<DeviceInfo>>,
}

impl AppState {
    /// Прочитать настройки, а при первом запуске создать их со значениями по умолчанию.
    pub fn load() -> Result<Self, ConfigError> {
        let config_path = Config::default_path()?;
        let first_run = !config_path.exists();
        let config = Config::load_or_create(&config_path)?;
        Ok(Self {
            config: Mutex::new(config),
            config_path,
            first_run,
            daemon: Mutex::new(Daemon::default()),
            transcriptions: Transcriptions::default(),
            hotkeys_paused: AtomicBool::new(false),
            last_state: Mutex::new(None),
            devices: Mutex::new(Vec::new()),
        })
    }

    /// Записать настройки на диск и запомнить их: так меняет конфиг трей.
    pub fn replace_config(&self, config: Config) -> Result<(), ConfigError> {
        config.save(&self.config_path)?;
        *lock(&self.config) = config;
        Ok(())
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        lock(&self.devices).clone()
    }

    pub fn set_devices(&self, devices: Vec<DeviceInfo>) {
        *lock(&self.devices) = devices;
    }

    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn first_run(&self) -> bool {
        self.first_run
    }

    pub fn hotkeys_paused(&self) -> bool {
        self.hotkeys_paused.load(Ordering::Relaxed)
    }

    pub fn set_hotkeys_paused(&self, paused: bool) {
        self.hotkeys_paused.store(paused, Ordering::Relaxed);
    }

    pub fn remember_state(&self, state: Option<DaemonState>) {
        *lock(&self.last_state) = state;
    }

    pub fn last_state(&self) -> Option<DaemonState> {
        *lock(&self.last_state)
    }

    pub fn stop_owned_daemon(&self) {
        let mut daemon = lock(&self.daemon);
        if daemon.is_ours() {
            // Сначала вежливо: демон закрывает микрофон и дописывает журнал.
            let _ = ipc::request(Command::Shutdown);
            daemon.stop();
        }
    }
}

// --- Состояние демона ---

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub daemon_running: bool,
    /// Демон запущен этим GUI: только его останавливает Quit.
    pub daemon_ours: bool,
    pub state: Option<DaemonState>,
    pub style: Option<String>,
    pub hotkeys_paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Разбор ответа `status`: демон может ещё не отдавать все поля, отсутствие — не ошибка.
pub fn parse_status(value: &serde_json::Value) -> (Option<DaemonState>, Option<String>) {
    let state = value
        .get("state")
        .and_then(|v| serde_json::from_value::<DaemonState>(v.clone()).ok());
    let style = value
        .get("style")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    (state, style)
}

/// Запрос к демону вне основного потока.
///
/// Синхронная команда Tauri выполняется в главном потоке, а чтение из сокета блокирующее:
/// занятый моделью демон отвечает не сразу, и окно замирало бы вместе с ним.
async fn ask_daemon(cmd: Command) -> Result<serde_json::Value, CommandError> {
    tauri::async_runtime::spawn_blocking(move || ipc::request(cmd))
        .await
        .map_err(|e| CommandError::new("internal", format!("запрос к демону прерван: {e}")))?
        .map_err(CommandError::from)
}

/// Команда демону, ответ которой интересен только фактом успеха.
async fn tell_daemon(cmd: Command) -> Result<(), CommandError> {
    ask_daemon(cmd).await?;
    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<Status, CommandError> {
    // Замок берётся и отпускается до ожидания: держать его через `await` нельзя.
    let daemon_ours = lock(&state.daemon).is_ours();
    match ask_daemon(Command::Status).await {
        Ok(value) => {
            let (daemon_state, style) = parse_status(&value);
            state.remember_state(daemon_state);
            Ok(Status {
                daemon_running: true,
                daemon_ours,
                state: daemon_state.or(Some(DaemonState::Idle)),
                style,
                hotkeys_paused: state.hotkeys_paused(),
                message: None,
                hint: None,
            })
        }
        Err(err) => {
            state.remember_state(None);
            Ok(Status {
                daemon_running: false,
                daemon_ours,
                state: None,
                style: None,
                hotkeys_paused: state.hotkeys_paused(),
                message: Some(err.message),
                hint: err.hint,
            })
        }
    }
}

fn mode_from(name: Option<&str>) -> Mode {
    match name {
        Some("command") => Mode::Command,
        _ => Mode::Dictation,
    }
}

#[tauri::command]
pub async fn record_start(mode: Option<String>, style: Option<String>) -> Result<(), CommandError> {
    tell_daemon(Command::RecordStart {
        mode: mode_from(mode.as_deref()),
        style,
    })
    .await
}

#[tauri::command]
pub async fn record_stop() -> Result<(), CommandError> {
    tell_daemon(Command::RecordStop).await
}

#[tauri::command]
pub async fn record_toggle(
    mode: Option<String>,
    style: Option<String>,
) -> Result<(), CommandError> {
    tell_daemon(Command::RecordToggle {
        mode: mode_from(mode.as_deref()),
        style,
    })
    .await
}

#[tauri::command]
pub async fn record_cancel() -> Result<(), CommandError> {
    tell_daemon(Command::RecordCancel).await
}

/// Стиль с панели идёт тем же путём, что и из трея: в файл настроек, демону и в трей,
/// иначе подсветка на панели не меняется, а после перезапуска выбор пропадает.
#[tauri::command]
pub fn set_style(app: AppHandle, style: String) -> Result<(), CommandError> {
    crate::tray::set_style(&app, &style);
    Ok(())
}

/// Разбор ответа `devices.list`: отсутствующий список — пустой, а не ошибка.
fn parse_devices(value: &serde_json::Value) -> Result<Vec<DeviceInfo>, CommandError> {
    let devices = value
        .get("devices")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    serde_json::from_value(devices)
        .map_err(|e| CommandError::new("daemon", format!("список устройств не разобран: {e}")))
}

/// Спросить у демона устройства из обычного потока: так делает подписка на события.
pub fn fetch_devices(state: &AppState) -> Result<Vec<DeviceInfo>, CommandError> {
    let devices = parse_devices(&ipc::request(Command::DevicesList)?)?;
    state.set_devices(devices.clone());
    Ok(devices)
}

#[tauri::command]
pub async fn list_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, CommandError> {
    let devices = parse_devices(&ask_daemon(Command::DevicesList).await?)?;
    state.set_devices(devices.clone());
    Ok(devices)
}

#[tauri::command]
pub async fn inject_text(text: String, mode: Option<String>) -> Result<(), CommandError> {
    let mode = mode.and_then(|m| serde_json::from_value::<OutputMode>(m.into()).ok());
    tell_daemon(Command::InjectText { text, mode }).await
}

#[tauri::command]
pub async fn reload_config() -> Result<(), CommandError> {
    tell_daemon(Command::ConfigReload).await
}

#[tauri::command]
pub fn start_daemon(state: State<'_, AppState>) -> Result<(), CommandError> {
    lock(&state.daemon).start().map_err(CommandError::from)
}

#[tauri::command]
pub fn stop_daemon(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.stop_owned_daemon();
    Ok(())
}

// --- Настройки ---

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.config()
}

/// Сообщить демону о новых настройках, не дожидаясь ответа.
///
/// Файл уже записан, так что для пользователя сохранение состоялось; незапущенный или
/// занятый демон не должен ни задерживать окно, ни превращать успех в ошибку.
fn notify_config_reload() {
    std::thread::spawn(|| {
        if let Err(err) = ipc::request(Command::ConfigReload) {
            tracing::info!(%err, "настройки сохранены, но демон о них ещё не знает");
        }
    });
}

#[tauri::command]
pub fn get_config_path(state: State<'_, AppState>) -> String {
    state.config_path().display().to_string()
}

#[tauri::command]
pub fn is_first_run(state: State<'_, AppState>) -> bool {
    state.first_run()
}

#[tauri::command]
pub fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
) -> Result<(), CommandError> {
    validate_config(&config)?;
    state.replace_config(config)?;
    notify_config_reload();
    #[cfg(not(target_os = "linux"))]
    if !state.hotkeys_paused() {
        crate::hotkeys::register(&app, &state.config().hotkeys);
    }
    crate::tray::refresh(&app);
    Ok(())
}

/// TOML настроек для экспорта: фронтенд показывает его и предлагает сохранить.
#[tauri::command]
pub fn export_config(state: State<'_, AppState>) -> Result<String, CommandError> {
    let dir = history::data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::new("config", e.to_string()))?;
    let path = dir.join("molva-config-export.toml");
    let text = state.config().to_toml_string()?;
    std::fs::write(&path, text).map_err(|e| CommandError::new("config", e.to_string()))?;
    Ok(path.display().to_string())
}

/// Импорт настроек из файла TOML: файл проверяется до перезаписи текущих настроек.
#[tauri::command]
pub fn import_config(state: State<'_, AppState>, path: String) -> Result<Config, CommandError> {
    let path = PathBuf::from(path);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CommandError::new("config", format!("{}: {e}", path.display())))?;
    let config = Config::from_toml_str(&path, &text)?;
    validate_config(&config)?;
    config.save(state.config_path())?;
    *lock(&state.config) = config.clone();
    notify_config_reload();
    Ok(config)
}

#[tauri::command]
pub fn reset_config(state: State<'_, AppState>) -> Result<Config, CommandError> {
    let config = Config::default();
    config.save(state.config_path())?;
    *lock(&state.config) = config.clone();
    notify_config_reload();
    Ok(config)
}

#[tauri::command]
pub fn hyprland_snippet(state: State<'_, AppState>) -> String {
    crate::hotkeys::hyprland_snippet(&state.config().hotkeys)
}

#[tauri::command]
pub fn set_autostart<R: Runtime>(app: AppHandle<R>, enabled: bool) -> Result<bool, CommandError> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| {
        CommandError::new("autostart", e.to_string()).with_hint(Some(
            "Проверьте права на запись в каталог автозапуска пользователя".into(),
        ))
    })?;
    Ok(manager.is_enabled().unwrap_or(enabled))
}

#[tauri::command]
pub fn get_autostart<R: Runtime>(app: AppHandle<R>) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn toggle_hotkeys_paused(state: State<'_, AppState>) -> bool {
    let paused = !state.hotkeys_paused();
    state.set_hotkeys_paused(paused);
    paused
}

// --- История ---

#[tauri::command]
pub fn history_list(
    state: State<'_, AppState>,
    filter: Option<Filter>,
) -> Result<Vec<Entry>, CommandError> {
    let entries = history::load_for(&state.config())?;
    Ok(history::filter_entries(
        &entries,
        &filter.unwrap_or_default(),
    ))
}

#[tauri::command]
pub fn history_apps(state: State<'_, AppState>) -> Result<Vec<String>, CommandError> {
    Ok(history::apps_of(&history::load_for(&state.config())?))
}

#[tauri::command]
pub fn history_delete(state: State<'_, AppState>, id: String) -> Result<bool, CommandError> {
    let id = Uuid::parse_str(&id)
        .map_err(|e| CommandError::new("history", format!("неверный идентификатор: {e}")))?;
    let path = history::journal_path(&state.config())?;
    Ok(history::delete(&path, id)?)
}

#[tauri::command]
pub fn history_clear(state: State<'_, AppState>) -> Result<(), CommandError> {
    let path = history::journal_path(&state.config())?;
    Ok(history::clear(&path)?)
}

#[tauri::command]
pub fn data_dir_path() -> Result<String, CommandError> {
    Ok(history::data_dir()?.display().to_string())
}

#[tauri::command]
pub fn open_path<R: Runtime>(app: AppHandle<R>, path: String) -> Result<(), CommandError> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| CommandError::new("opener", e.to_string()))
}

// --- Статистика ---

#[tauri::command]
pub fn stats_summary(
    state: State<'_, AppState>,
    range_days: Option<u32>,
) -> Result<StatsSummary, CommandError> {
    let config = state.config();
    let entries = history::load_for(&config)?;
    Ok(stats::summary(
        &entries,
        Utc::now(),
        &config.stats,
        range_days.unwrap_or(7),
    ))
}

#[tauri::command]
pub fn stats_export_csv(
    state: State<'_, AppState>,
    range_days: Option<u32>,
) -> Result<String, CommandError> {
    let summary = stats_summary(state, range_days)?;
    let dir = history::data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::new("history", e.to_string()))?;
    let path = dir.join(format!("molva-stats-{}.csv", Utc::now().format("%Y-%m-%d")));
    std::fs::write(&path, stats::series_to_csv(&summary.series))
        .map_err(|e| CommandError::new("history", e.to_string()))?;
    Ok(path.display().to_string())
}

// --- Разбор файлов ---

/// Прогресс разбора: одна строка stderr CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeProgress {
    pub id: String,
    pub line: String,
}

#[tauri::command]
pub async fn transcribe_file<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    path: String,
) -> Result<serde_json::Value, CommandError> {
    let handle = app.clone();
    let task_id = id.clone();
    // Разбор долгий: уводим его с потока команд, чтобы интерфейс не подвисал.
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let emitter = handle.clone();
        let progress_id = task_id.clone();
        let sink: sidecar::ProgressSink = std::sync::Arc::new(move |line: &str| {
            let _ = emitter.emit(
                "molva://transcribe",
                TranscribeProgress {
                    id: progress_id.clone(),
                    line: line.to_string(),
                },
            );
        });
        sidecar::transcribe(
            &state.transcriptions,
            &task_id,
            std::path::Path::new(&path),
            sink,
        )
        .map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::new("sidecar", format!("задача разбора прервана: {e}")))?
}

#[tauri::command]
pub fn transcribe_cancel(state: State<'_, AppState>, id: String) -> bool {
    state.transcriptions.cancel(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_passes_validation() {
        assert!(validate_config(&Config::default()).is_ok());
    }

    #[test]
    fn unknown_output_mode_names_the_field_and_the_allowed_values() {
        let mut config = Config::default();
        config.output.mode = "телепатия".into();
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.field, "output.mode");
        assert!(err.allowed.contains(&"clipboard".to_string()));
        let shown: CommandError = err.into();
        assert_eq!(shown.kind, "validation");
        assert!(shown.hint.unwrap().contains("clipboard"));
    }

    #[test]
    fn gain_outside_the_range_is_rejected() {
        let mut config = Config::default();
        config.audio.gain = 0.0;
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.field, "audio.gain");
    }

    #[test]
    fn enabled_llm_without_an_address_is_rejected() {
        let mut config = Config::default();
        config.llm.enabled = true;
        config.llm.base_url = "  ".into();
        assert_eq!(validate_config(&config).unwrap_err().field, "llm.base_url");
    }

    #[test]
    fn disabled_llm_without_an_address_is_fine() {
        let mut config = Config::default();
        config.llm.base_url = String::new();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn interface_language_is_limited_to_supported_ones() {
        let config = Config {
            ui_language: "de".into(),
            ..Config::default()
        };
        assert_eq!(validate_config(&config).unwrap_err().field, "ui_language");
    }

    #[test]
    fn local_providers_need_no_confirmation_cloud_ones_do() {
        assert!(!is_cloud_provider("ollama"));
        assert!(!is_cloud_provider("lmstudio"));
        assert!(is_cloud_provider("openai"));
        assert!(is_cloud_provider("groq"));
    }

    #[test]
    fn status_without_fields_is_not_an_error() {
        let (state, style) = parse_status(&serde_json::json!({}));
        assert_eq!(state, None);
        assert_eq!(style, None);
    }

    #[test]
    fn status_reports_state_and_style() {
        let (state, style) =
            parse_status(&serde_json::json!({"state": "transcribing", "style": "formal"}));
        assert_eq!(state, Some(DaemonState::Transcribing));
        assert_eq!(style.as_deref(), Some("formal"));
    }

    #[test]
    fn unavailable_daemon_becomes_a_distinct_error_kind_with_a_next_step() {
        let err: CommandError = IpcClientError::NotRunning {
            path: "/run/user/1000/molva.sock".into(),
            reason: "нет файла".into(),
        }
        .into();
        assert_eq!(err.kind, "daemon_unavailable");
        assert!(err.hint.is_some());
    }

    #[test]
    fn mode_defaults_to_dictation() {
        assert_eq!(mode_from(None), Mode::Dictation);
        assert_eq!(mode_from(Some("command")), Mode::Command);
        assert_eq!(mode_from(Some("чепуха")), Mode::Dictation);
    }
}
