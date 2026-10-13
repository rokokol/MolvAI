// SPDX-License-Identifier: MIT
//! Commands available to the frontend, and the shared state of the application
//!
//! Everything that Rust can compute is computed here: the frontend receives ready structures
//! and does not repeat the logic of the core in TypeScript

// Tauri sets the signature of a command: `State` and the arguments parsed from JSON arrive
// by value, and `invoke_handler` cannot accept a reference
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use chrono::Utc;
use molva_core::app::models::{self, DownloadProgress};
use molva_core::config::ConfigError;
use molva_core::domain::entry::Mode;
use molva_core::domain::{DeviceInfo, Entry, OutputMode};
use molva_core::ipc::{Command, DaemonState};
use molva_core::Config;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

use crate::history::{self, Filter, HistoryError};
use crate::i18n;
use crate::ipc::{self, IpcClientError};
use crate::lock;
use crate::sidecar::{self, Daemon, DaemonFailure, SidecarError, Transcriptions};
use crate::stats::{self, StatsSummary};

/// A command error in the form that the interface shows: reason, hint, field
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    /// `daemon_unavailable` | `daemon` | `config` | `validation` | `history` | `sidecar`.
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The settings form field that the validation error belongs to
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
        Self::new(kind, err.user_message()).with_hint(hint)
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
            SidecarError::NotFound | SidecarError::Spawn { .. } => {
                Some(i18n::t("files.cliMissingHint"))
            }
            _ => None,
        };
        // The window tells a cancel from a failure by the kind, never by the wording
        let kind = if matches!(err, SidecarError::Cancelled) {
            "cancelled"
        } else {
            "sidecar"
        };
        Self::new(kind, err.to_string()).with_hint(hint)
    }
}

/// Settings validation error: field, reason, allowed values
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
    pub allowed: Vec<String>,
}

impl From<ValidationError> for CommandError {
    fn from(err: ValidationError) -> Self {
        let hint = (!err.allowed.is_empty())
            .then(|| i18n::tv("validation.allowed", &[("values", &err.allowed.join(", "))]));
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
        message: i18n::tv("validation.invalidValue", &[("value", value)]),
        allowed: allowed.iter().map(ToString::to_string).collect(),
    })
}

fn in_range(field: &str, value: f32, min: f32, max: f32) -> Result<(), ValidationError> {
    if value >= min && value <= max {
        return Ok(());
    }
    Err(ValidationError {
        field: field.into(),
        message: i18n::tv("validation.outOfRange", &[("value", &value.to_string())]),
        allowed: vec![i18n::tv(
            "validation.range",
            &[("min", &min.to_string()), ("max", &max.to_string())],
        )],
    })
}

/// Check the settings before the write to disk: the user sees the field and the allowed values
pub fn validate_config(config: &Config) -> Result<(), ValidationError> {
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
            message: i18n::t("validation.zeroDuration"),
            allowed: vec![i18n::t("validation.minDuration")],
        });
    }
    if config.stt.model.trim().is_empty() {
        return Err(ValidationError {
            field: "stt.model".into(),
            message: i18n::t("validation.noModel"),
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
            message: i18n::t("validation.noBaseUrl"),
            allowed: vec!["http://localhost:11434/v1".into()],
        });
    }
    // The other rules live in the core: the daemon checks the same file with the same function
    if let Err(issues) = config.validate() {
        if let Some(issue) = issues.into_iter().next() {
            return Err(ValidationError {
                field: issue.key,
                message: issue.message,
                allowed: Vec::new(),
            });
        }
    }
    Ok(())
}

/// Whether the provider is a cloud one: the interface requires an explicit confirmation for it
pub fn is_cloud_provider(provider: &str) -> bool {
    !matches!(provider, "ollama" | "lmstudio" | "custom")
}

/// Built-in post-processing styles; the menu label of each is `style.<id>` in the dictionaries
// The ids match `molva_core::app::styles::builtin`: the daemon accepts only these, and an
// unknown id would silently fall back to the default style
pub const BUILTIN_STYLES: [&str; 6] =
    ["verbatim", "cleanup", "messenger", "mail", "code", "formal"];

/// Built-in styles plus the user's own from the config, without repeated ids
pub fn builtin_style_options(config: &Config) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = BUILTIN_STYLES
        .iter()
        .map(|id| (id.to_string(), i18n::t(&format!("style.{id}"))))
        .collect();
    for style in &config.style.custom {
        if options.iter().any(|(id, _)| id == &style.id) {
            continue;
        }
        options.push((style.id.clone(), style.name.clone()));
    }
    options
}

/// A style in a form that suits the drop-down lists of the interface
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

/// Shared state: settings, the daemon that we started, file transcriptions, hotkey pause
#[derive(Debug)]
pub struct AppState {
    config: Mutex<Config>,
    config_path: PathBuf,
    /// There was no settings file: the interface opens on the Settings tab at once
    first_run: bool,
    daemon: Mutex<Daemon>,
    transcriptions: Transcriptions,
    hotkeys_paused: AtomicBool,
    last_state: Mutex<Option<DaemonState>>,
    /// The device list is cached: the tray menu is rebuilt often, and a daemon query is not cheap
    devices: Mutex<Vec<DeviceInfo>>,
}

impl AppState {
    /// Read the settings, and on the first start create them with the default values
    pub fn load() -> Result<Self, ConfigError> {
        let config_path = Config::default_path()?;
        let first_run = !config_path.exists();
        let config = Config::load_or_create(&config_path)?;
        i18n::apply(&config.ui_language);
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

    /// Write the settings to disk and keep them; every settings change goes through here, so
    /// the interface language follows the new `ui_language` at once
    pub fn replace_config(&self, config: Config) -> Result<(), ConfigError> {
        config.save(&self.config_path)?;
        i18n::apply(&config.ui_language);
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
            // Ask politely first: the daemon closes the microphone and finishes the journal
            let _ = ipc::request(Command::Shutdown);
            daemon.stop();
        }
    }
}

// --- Daemon state ---

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub daemon_running: bool,
    /// This GUI started the daemon: only Quit stops such a daemon
    pub daemon_ours: bool,
    pub state: Option<DaemonState>,
    pub style: Option<String>,
    pub hotkeys_paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The daemon has not opened the socket yet, because it downloads model weights
    /// (`stt.auto_pull`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<DownloadProgress>,
    /// The daemon that this GUI started has crashed: exit code and its last stderr line
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<String>,
}

/// Status for the case when the daemon did not answer
fn unreachable_status(
    config: &Config,
    daemon_ours: bool,
    failure: Option<&DaemonFailure>,
    hotkeys_paused: bool,
    err: CommandError,
) -> Status {
    let download = models::active_download(
        config,
        config.stt.model.trim(),
        std::time::SystemTime::now(),
    );
    let exit_reason = failure.map(|failure| {
        let how = failure.code.map_or_else(
            || i18n::t("daemon.exitSignal"),
            |code| i18n::tv("daemon.exitCode", &[("code", &code.to_string())]),
        );
        if failure.last_line.is_empty() {
            i18n::tv("daemon.exited", &[("how", &how)])
        } else {
            i18n::tv(
                "daemon.exitedLine",
                &[("how", &how), ("line", &failure.last_line)],
            )
        }
    });
    Status {
        daemon_running: false,
        daemon_ours,
        state: None,
        style: None,
        hotkeys_paused,
        message: Some(err.message),
        hint: err.hint,
        download,
        exit_reason,
    }
}

/// Parse the `status` response: the daemon may not give all fields yet,
/// and a missing field is no error
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

/// Query the daemon outside the main thread
///
/// A synchronous Tauri command runs on the main thread, and reading from the socket blocks.
/// A daemon that the model keeps busy does not answer at once, and the window would freeze
/// with it
async fn ask_daemon(cmd: Command) -> Result<serde_json::Value, CommandError> {
    tauri::async_runtime::spawn_blocking(move || ipc::request(cmd))
        .await
        .map_err(|e| {
            CommandError::new(
                "internal",
                format!("the daemon request was interrupted: {e}"),
            )
        })?
        .map_err(CommandError::from)
}

/// A command to the daemon where only the success of the response matters
async fn tell_daemon(cmd: Command) -> Result<(), CommandError> {
    ask_daemon(cmd).await?;
    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<Status, CommandError> {
    // The lock is taken and released before the wait: we must not hold it across `await`
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
                download: None,
                exit_reason: None,
            })
        }
        Err(err) => {
            state.remember_state(None);
            let failure = lock(&state.daemon).failure();
            Ok(unreachable_status(
                &state.config(),
                daemon_ours,
                failure.as_ref(),
                state.hotkeys_paused(),
                err,
            ))
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

/// A style from the dashboard takes the same path as one from the tray: to the settings file,
/// to the daemon and to the tray. Otherwise the highlight on the dashboard does not change,
/// and the choice is lost after a restart
#[tauri::command]
pub fn set_style(app: AppHandle, style: String) -> Result<(), CommandError> {
    crate::tray::set_style(&app, &style);
    Ok(())
}

/// Parse the `devices.list` response: a missing list is an empty list, not an error
fn parse_devices(value: &serde_json::Value) -> Result<Vec<DeviceInfo>, CommandError> {
    let devices = value
        .get("devices")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    serde_json::from_value(devices).map_err(|e| {
        CommandError::new(
            "daemon",
            format!("the device list could not be parsed: {e}"),
        )
    })
}

/// Ask the daemon for the devices from a normal thread: the event subscription does this
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

// --- Settings ---

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.config()
}

/// Tell the daemon about the new settings without waiting for the response
///
/// The file is already written, so for the user the save has happened. A daemon that is not
/// running, or is busy, must neither delay the window nor turn the success into an error
fn notify_config_reload() {
    std::thread::spawn(|| {
        if let Err(err) = ipc::request(Command::ConfigReload) {
            tracing::info!(%err, "settings saved, but the daemon does not know about them yet");
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
    let hotkeys_changed = state.config().hotkeys != config.hotkeys;
    state.replace_config(config)?;
    notify_config_reload();
    if hotkeys_changed {
        refresh_hyprland_binds(&state.config().hotkeys);
    }
    #[cfg(not(target_os = "linux"))]
    if !state.hotkeys_paused() {
        crate::hotkeys::register(&app, &state.config().hotkeys);
    }
    crate::tray::refresh(&app);
    Ok(())
}

/// Settings TOML for export: the frontend shows it and offers to save it
#[tauri::command]
pub fn export_config(state: State<'_, AppState>) -> Result<String, CommandError> {
    let dir = history::data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::new("config", e.to_string()))?;
    let path = dir.join("molva-config-export.toml");
    let text = state.config().to_toml_string()?;
    std::fs::write(&path, text).map_err(|e| CommandError::new("config", e.to_string()))?;
    Ok(path.display().to_string())
}

/// Import settings from a TOML file: the file is checked before it replaces the current settings
#[tauri::command]
pub fn import_config(state: State<'_, AppState>, path: String) -> Result<Config, CommandError> {
    let path = PathBuf::from(path);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CommandError::new("config", format!("{}: {e}", path.display())))?;
    let config = Config::from_toml_str(&path, &text)?;
    validate_config(&config)?;
    state.replace_config(config.clone())?;
    notify_config_reload();
    Ok(config)
}

#[tauri::command]
pub fn reset_config(state: State<'_, AppState>) -> Result<Config, CommandError> {
    let config = Config::default();
    state.replace_config(config.clone())?;
    notify_config_reload();
    Ok(config)
}

/// The interface language, resolved on the Rust side only so the window and the tray agree
#[tauri::command]
pub fn ui_language() -> &'static str {
    i18n::current().code()
}

fn hyprland_path() -> Result<PathBuf, CommandError> {
    crate::hotkeys::hyprland_path()
        .ok_or_else(|| CommandError::new("hyprland", "neither XDG_CONFIG_HOME nor HOME is set"))
}

/// Where the Hyprland binds file is, the line that includes it, and whether it exists
#[tauri::command]
pub fn hyprland_binds() -> Result<crate::hotkeys::HyprlandBinds, CommandError> {
    Ok(crate::hotkeys::hyprland_binds(&hyprland_path()?))
}

/// Write the Hyprland binds from the current hotkeys, then reload Hyprland
#[tauri::command]
pub fn write_hyprland_binds(
    state: State<'_, AppState>,
) -> Result<crate::hotkeys::HyprlandBinds, CommandError> {
    let binds = crate::hotkeys::write_hyprland_binds(&hyprland_path()?, &state.config().hotkeys)
        .map_err(|message| CommandError::new("hyprland", message))?;
    crate::hotkeys::reload_hyprland();
    Ok(binds)
}

/// After a hotkey change, rewrite the binds file if the user has one: it must not keep the old
/// keys. Without a file there is nothing to keep in step, and nothing is created
fn refresh_hyprland_binds(hotkeys: &molva_core::config::HotkeysConfig) {
    let Some(path) = crate::hotkeys::hyprland_path().filter(|path| path.exists()) else {
        return;
    };
    match crate::hotkeys::write_hyprland_binds(&path, hotkeys) {
        Ok(_) => crate::hotkeys::reload_hyprland(),
        Err(err) => tracing::warn!(%err, "the Hyprland binds file was not rewritten"),
    }
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
        CommandError::new("autostart", e.to_string())
            .with_hint(Some(i18n::t("settings.autostartHint")))
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

// --- History ---

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
        .map_err(|e| CommandError::new("history", format!("invalid id: {e}")))?;
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

// --- Statistics ---

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

// --- File transcription ---

/// Transcription progress: one stderr line of the CLI
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
    // Transcription is slow: we move it off the command thread so the interface does not hang
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
    .map_err(|e| {
        CommandError::new(
            "sidecar",
            format!("the transcription task was interrupted: {e}"),
        )
    })?
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
        config.output.mode = "telepathy".into();
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.field, "output.mode");
        assert!(err.allowed.contains(&"clipboard".to_string()));
        let shown: CommandError = err.into();
        assert_eq!(shown.kind, "validation");
        assert!(shown.hint.unwrap().contains("clipboard"));
    }

    #[test]
    fn rules_of_the_core_are_checked_before_saving() {
        let mut config = Config::default();
        config.stt.models_mirror = "hf-mirror.com/whisper".into();
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.field, "stt.models_mirror");
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
        for ok in ["auto", "ru", "en"] {
            let config = Config {
                ui_language: ok.into(),
                ..Config::default()
            };
            assert!(validate_config(&config).is_ok(), "{ok}");
        }
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
            reason: "no file".into(),
        }
        .into();
        assert_eq!(err.kind, "daemon_unavailable");
        assert!(err.hint.is_some());
    }

    #[test]
    fn an_unreachable_daemon_reports_the_model_being_downloaded() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stt.model_path = directory.path().display().to_string();
        let info = models::find(config.stt.model.trim()).unwrap();
        std::fs::write(
            models::partial_path(directory.path(), info.file_name),
            vec![0u8; 1024],
        )
        .unwrap();

        let status = unreachable_status(&config, false, None, false, CommandError::new("x", "no"));
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["daemon_running"], false);
        assert_eq!(json["download"]["model"], info.name);
        assert_eq!(json["download"]["downloaded"], 1024);
        assert_eq!(json["download"]["total"], info.size_bytes);
    }

    #[test]
    fn a_crashed_daemon_explains_itself_in_the_status() {
        let config = Config::default();
        let failure = DaemonFailure {
            code: Some(5),
            last_line: "error: network unreachable".into(),
        };
        let status = unreachable_status(
            &config,
            true,
            Some(&failure),
            false,
            CommandError::new("daemon_unavailable", "daemon does not answer"),
        );
        let reason = status.exit_reason.unwrap();
        assert!(reason.contains("network unreachable"), "{reason}");
        assert!(reason.contains('5'), "{reason}");
    }

    #[test]
    fn without_a_download_the_status_has_no_download_field() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stt.model_path = directory.path().display().to_string();
        let status = unreachable_status(&config, false, None, false, CommandError::new("x", "no"));
        let json = serde_json::to_value(&status).unwrap();
        assert!(json.get("download").is_none(), "{json}");
        assert!(json.get("exit_reason").is_none(), "{json}");
    }

    #[test]
    fn mode_defaults_to_dictation() {
        assert_eq!(mode_from(None), Mode::Dictation);
        assert_eq!(mode_from(Some("command")), Mode::Command);
        assert_eq!(mode_from(Some("nonsense")), Mode::Dictation);
    }
}
