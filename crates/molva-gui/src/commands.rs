// SPDX-License-Identifier: MIT
//! Commands available to the frontend, and the shared state of the application
//!
//! Everything that Rust can compute is computed here: the frontend receives ready structures
//! and does not repeat the logic of the core in TypeScript

// Tauri sets the signature of a command: `State` and the arguments parsed from JSON arrive
// by value, and `invoke_handler` cannot accept a reference
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use molva_core::app::engine::{build_stt_with, EngineChoice, EngineError};
use molva_core::app::hotkeys::HotkeySpec;
use molva_core::app::models::{self, DownloadProgress};
use molva_core::app::probe::{LlmProbeReport, ProbeFailure, ProbeOutcome, SttProbeReport};
use molva_core::app::secrets::{self, SecretError, SecretStore};
// The summary is the core's own: a second copy here drifted from it and lost the model cost
use molva_core::app::stats::{self, StatsSummary};
use molva_core::config::{ConfigError, HotkeysConfig};
use molva_core::domain::entry::{Mode, Source};
use molva_core::domain::llm::LlmError;
use molva_core::domain::{DeviceInfo, Entry, OutputMode};
use molva_core::infra::audio::list_input_devices;
use molva_core::ipc::{
    Command, DaemonState, ErrorCode, FileTranscriptionRequest, IpcError, WorkerJob,
};
use molva_core::Config;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

use crate::history::{self, Filter, HistoryError};
use crate::i18n;
use crate::ipc::{self, IpcClientError};
use crate::lock;
use crate::media_server::MediaServer;
use crate::paths::{self, MediaKind, PathSuggestion};
use crate::sidecar::{self, Daemon, DaemonFailure, SidecarError, READY_STEP};

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
            // The binary that the window starts is the one beside it: a missing one is a
            // broken installation, and the next step is to install it
            SidecarError::NotFound | SidecarError::Spawn { .. } => {
                Some(i18n::t("files.cliMissingHint"))
            }
            // A start that takes minutes is normally a weight download, and the dashboard
            // already shows its progress: the user needs to know where to look
            SidecarError::NotReady { .. } => Some(i18n::t("daemon.startingHint")),
            SidecarError::Startup { .. } => Some(i18n::t("daemon.startFailedHint")),
        };
        Self::new("sidecar", err.to_string()).with_hint(hint)
    }
}

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
        &["auto", "whisper-cpp", "transcribe-cpp", "remote-openai"],
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
        &["keyring", "file", "env", "plain", "none"],
    )?;
    one_of(
        "llm.data_collection",
        &config.llm.data_collection,
        &["allow", "deny"],
    )?;
    one_of(
        "stt.remote.api_key_source",
        &config.stt.remote.api_key_source,
        &["keyring", "env", "none"],
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
            // The names come from the catalog, so a hint never lists a model that cannot be
            // pulled or one of another engine
            allowed: models::catalog().iter().map(|m| m.name.clone()).collect(),
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

/// Built-in post-processing styles; the menu label of each is `style.<id>` in the dictionaries
// The ids match `molva_core::app::styles::builtin`. An id outside this list and the custom
// styles resolves to the default style
pub const BUILTIN_STYLES: [&str; 3] = ["verbatim", "cleanup", "formal"];

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

/// The built-in styles alone: the editor lists them as defaults and keeps them out of the
/// custom list, where a repeated id would read as an edit of a built-in style
#[tauri::command]
pub fn builtin_styles() -> Vec<StyleOption> {
    BUILTIN_STYLES
        .iter()
        .map(|id| StyleOption {
            id: (*id).to_string(),
            name: i18n::t(&format!("style.{id}")),
        })
        .collect()
}

// --- Recognition: the catalog, the capabilities and the keys of the providers ---

/// One set of weights as the settings page offers it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SttModelOption {
    /// The value of `stt.model`
    pub name: String,
    /// The recognizer that can load the file: `whisper-cpp` or `transcribe-cpp`
    pub engine: String,
    /// The language of a model that is not multilingual
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub size_bytes: u64,
    /// The weights are already on disk, so the choice needs no download
    pub installed: bool,
}

/// The catalog from `models.toml`, with the weights that are already on disk
///
/// The list is not repeated in TypeScript: the file that the flake and the CLI read decides
/// which model belongs to which engine
pub fn stt_model_options(config: &Config) -> Vec<SttModelOption> {
    models::catalog()
        .iter()
        .map(|info| SttModelOption {
            name: info.name.clone(),
            engine: info.engine.clone(),
            language: info.language.clone(),
            size_bytes: info.size_bytes,
            installed: models::installed_path(config, &info.name).is_ok_and(|path| path.exists()),
        })
        .collect()
}

#[tauri::command]
pub async fn stt_models(state: State<'_, AppState>) -> Result<Vec<SttModelOption>, CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || stt_model_options(&config))
        .await
        .map_err(|error| CommandError::new("internal", error.to_string()))
}

/// Whether this build can recognize through a remote API
///
/// The question is put to the engine factory rather than answered from a list: a build that
/// drops the cloud engine answers `false` here on its own. A missing address or key is not this
/// question — the factory names it in `EngineError::RemoteConfig`, and the settings page reports
/// that from the probe instead of hiding the engine.
///
/// The settings page no longer asks: every build of this version carries the cloud engine, and
/// the page shows its fields and the consent warning instead of hiding them behind an answer
#[cfg(test)]
pub fn remote_stt_available(config: &Config) -> bool {
    let mut probe = config.clone();
    probe.stt.engine = "remote-openai".into();
    !matches!(
        build_stt_with(&probe, &EngineChoice::default()),
        Err(EngineError::NotAvailable(_))
    )
}


/// The key of one provider as the page may show it: never the value
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderKeyStatus {
    /// An entry exists in the chosen store
    pub present: bool,
    /// The entry name, so the user can find it in the OS key store
    pub entry: String,
    /// Why there is no answer: the store refused, and that differs from "no key"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// State of the key entry in `store`; the value never leaves the store
pub fn key_status(store: &dyn SecretStore, provider: &str) -> ProviderKeyStatus {
    let entry = secrets::llm_key_entry(provider);
    match store.get(&entry) {
        Ok(Some(value)) => ProviderKeyStatus {
            present: !value.trim().is_empty(),
            entry,
            detail: None,
        },
        Ok(None) => ProviderKeyStatus {
            present: false,
            entry,
            detail: None,
        },
        Err(error) => ProviderKeyStatus {
            present: false,
            entry,
            detail: Some(error.to_string()),
        },
    }
}

/// Store a pasted key. The value goes to the store and nowhere else: the answer carries the
/// entry name and whether it is there, so no log line and no error message can echo it
pub fn store_key(
    store: &dyn SecretStore,
    provider: &str,
    key: &str,
) -> Result<ProviderKeyStatus, CommandError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(CommandError::new("key", i18n::t("settings.llm.keyEmpty")));
    }
    let entry = secrets::llm_key_entry(provider);
    store.set(&entry, key).map_err(store_error)?;
    Ok(ProviderKeyStatus {
        present: true,
        entry,
        detail: None,
    })
}

pub fn remove_key(
    store: &dyn SecretStore,
    provider: &str,
) -> Result<ProviderKeyStatus, CommandError> {
    let entry = secrets::llm_key_entry(provider);
    store.delete(&entry).map_err(store_error)?;
    Ok(key_status(store, provider))
}

fn store_error(error: SecretError) -> CommandError {
    CommandError::new("key", error.to_string())
        .with_hint(Some(i18n::t("settings.llm.keyStoreHint")))
}

/// A store by the name of the source; `plain` and `env` keep no entry, so the page never asks
fn key_store(source: &str) -> Box<dyn SecretStore> {
    secrets::store_for_source(source)
}

/// How long the settings page waits for the OS key store before saying it does not answer
const KEYRING_TIMEOUT: Duration = Duration::from_secs(3);

/// Ask the store with a bound: a locked collection may wait for an unlock prompt that Hyprland
/// never shows, and the settings page must show a reason instead of staying in "checking"
fn key_status_with_timeout(provider: &str, source: &str) -> ProviderKeyStatus {
    let (tx, rx) = mpsc::channel();
    let provider_for_thread = provider.to_string();
    let source_for_thread = source.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(key_status(
            key_store(&source_for_thread).as_ref(),
            &provider_for_thread,
        ));
    });
    rx.recv_timeout(KEYRING_TIMEOUT).unwrap_or_else(|_| ProviderKeyStatus {
        present: false,
        entry: secrets::llm_key_entry(provider),
        detail: Some(
            "the key store did not answer within 3 s; unlock the keyring or use the environment variable source"
                .into(),
        ),
    })
}

#[tauri::command]
pub async fn provider_key_status(provider: String, source: String) -> ProviderKeyStatus {
    // The OS key store can block while its service starts or asks for an unlock. A synchronous
    // Tauri command would freeze the window on Settings open, so the call runs off the UI thread.
    tauri::async_runtime::spawn_blocking(move || key_status_with_timeout(&provider, &source))
        .await
        .unwrap_or(ProviderKeyStatus {
            present: false,
            entry: "llm-api-key".into(),
            detail: Some("the key store check crashed; use the environment variable source".into()),
        })
}

#[tauri::command]
pub async fn store_provider_key(
    provider: String,
    source: String,
    key: String,
) -> Result<ProviderKeyStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        store_key(key_store(&source).as_ref(), &provider, &key)
    })
    .await
    .map_err(|error| CommandError::new("internal", error.to_string()))?
}

#[tauri::command]
pub async fn remove_provider_key(
    provider: String,
    source: String,
) -> Result<ProviderKeyStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(move || remove_key(key_store(&source).as_ref(), &provider))
        .await
        .map_err(|error| CommandError::new("internal", error.to_string()))?
}

/// The providers the settings page offers, with the URL and the local flag of each
#[tauri::command]
pub fn llm_providers() -> Vec<crate::providers::LlmProviderOption> {
    crate::providers::llm_providers()
}

// --- Diagnostics: the test buttons of the settings page ---
//
// Both tests work from the draft, never from the saved file: the person changes a provider or a
// path and asks "does this work now" before the settings are written. Neither test saves
// anything, and neither one opens the microphone

/// A draft failure as the settings page shows it, with the next step of each kind
fn probe_failure(error: ProbeFailure) -> CommandError {
    match error {
        ProbeFailure::Validation { field, key, value } => {
            let message = if value.is_empty() {
                i18n::t(&key)
            } else {
                i18n::tv(&key, &[("value", &value)])
            };
            let hint = (field == "llm.model").then(|| i18n::t("settings.llm.modelHint"));
            CommandError {
                kind: "validation".into(),
                message,
                hint,
                field: Some(field),
            }
        }
        ProbeFailure::Consent => CommandError::new("consent", i18n::t("error.cloudConsent"))
            .with_hint(Some(i18n::t("settings.llm.probeConsentHint"))),
        ProbeFailure::Llm(error) => {
            let hint = match &error {
                LlmError::Auth => Some(i18n::t("settings.llm.probeAuthHint")),
                LlmError::Timeout(seconds) => Some(i18n::tv(
                    "settings.llm.probeTimeoutHint",
                    &[("seconds", &seconds.to_string())],
                )),
                LlmError::Unavailable(_) => Some(i18n::t("settings.llm.probeUnavailableHint")),
                LlmError::BadResponse(_) | LlmError::Disabled | LlmError::Consent => None,
            };
            CommandError::new("llm", error.to_string()).with_hint(hint)
        }
    }
}

fn probe_report<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, CommandError> {
    let outcome: ProbeOutcome<T> = serde_json::from_value(value)
        .map_err(|err| CommandError::new("internal", err.to_string()))?;
    match (outcome.report, outcome.error) {
        (Some(report), _) => Ok(report),
        (_, Some(error)) => Err(probe_failure(error)),
        _ => Err(CommandError::new(
            "internal",
            "the daemon returned no probe answer",
        )),
    }
}

/// Test the draft model configuration through the daemon, which holds the key
#[tauri::command]
pub async fn test_llm(config: Config) -> Result<LlmProbeReport, CommandError> {
    let value = tauri::async_runtime::spawn_blocking(move || {
        ipc::request(Command::LlmProbe {
            config: Box::new(config),
        })
    })
    .await
    .map_err(|err| CommandError::new("internal", err.to_string()))?
    .map_err(|err| CommandError::new("daemon", err.user_message()))?;
    probe_report(value)
}

/// Test the draft recognition settings through the daemon; no device is opened
#[tauri::command]
pub async fn test_stt(config: Config) -> Result<SttProbeReport, CommandError> {
    let value = tauri::async_runtime::spawn_blocking(move || {
        ipc::request(Command::SttProbe {
            config: Box::new(config),
        })
    })
    .await
    .map_err(|err| CommandError::new("internal", err.to_string()))?
    .map_err(|err| CommandError::new("daemon", err.user_message()))?;
    probe_report(value)
}

/// A hotkey as the settings page shows it after a capture
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotkeyCheck {
    /// The text to store in `hotkeys.*`
    pub value: String,
}

/// Validate a captured combination with the parser the daemon uses
///
/// The keys that are known and the way they are spelled live in `molva_core::app::hotkeys`:
/// a second table here would accept a combination that the daemon cannot bind
#[tauri::command]
pub fn check_hotkey(value: String) -> Result<HotkeyCheck, CommandError> {
    HotkeySpec::parse(&value)
        .map(|_| HotkeyCheck { value })
        .map_err(|error| CommandError::new("hotkey", error.to_string()))
}

/// Shared state: settings, the daemon this process started, file transcriptions, hotkey pause
#[derive(Debug)]
pub struct AppState {
    config: Mutex<Config>,
    config_path: PathBuf,
    /// The config resolves into the Nix store: writes go to a runtime file beside it
    managed: bool,
    /// There was no settings file: the interface opens on the Settings tab at once
    first_run: bool,
    daemon: Mutex<Daemon>,
    last_state: Mutex<Option<DaemonState>>,
    /// The device list is cached: the tray menu is rebuilt often, and a daemon query is not cheap
    devices: Mutex<Vec<DeviceInfo>>,
    /// The loopback server that streams media to the player; it starts on the first preview
    media: Mutex<Option<MediaServer>>,
}

impl AppState {
    /// The port and token that play a file, starting the server on first use
    ///
    /// A failure to bind is a real error, not a silent one: the caller reports it instead of
    /// handing the player a URL that answers nothing.
    fn media_server(&self) -> Result<std::sync::MutexGuard<'_, Option<MediaServer>>, CommandError> {
        let mut guard = lock(&self.media);
        if guard.is_none() {
            *guard = Some(MediaServer::start().map_err(|e| {
                CommandError::new("file", format!("cannot start the media server: {e}"))
            })?);
        }
        Ok(guard)
    }

    /// Read the settings, and on the first start create them with the default values
    pub fn load() -> Result<Self, ConfigError> {
        let config_path = Config::default_path()?;
        let managed = Config::is_managed(&config_path);
        let first_run = !config_path.exists() && !managed;
        let config = Config::load_or_create(&config_path)?;
        i18n::apply(&config.ui_language);
        Ok(Self {
            config: Mutex::new(config),
            config_path,
            managed,
            first_run,
            daemon: Mutex::new(Daemon::default()),
            last_state: Mutex::new(None),
            devices: Mutex::new(Vec::new()),
            media: Mutex::new(None),
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

    /// Keep the style that the daemon published; the daemon has already written the settings
    pub fn set_style(&self, style: &str) {
        lock(&self.config).style.default = style.to_string();
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn managed(&self) -> bool {
        self.managed
    }

    pub fn first_run(&self) -> bool {
        self.first_run
    }

    /// Only the hotkey table, not a clone of every setting
    pub fn hotkeys(&self) -> HotkeysConfig {
        lock(&self.config).hotkeys.clone()
    }

    pub fn hotkeys_paused(&self) -> bool {
        Config::hotkeys_paused()
    }

    /// A compositor bind calls `molva --hotkey`, which reads the same marker
    pub fn set_hotkeys_paused(&self, paused: bool) -> Result<(), ConfigError> {
        Config::set_hotkeys_paused(paused)
    }

    pub fn remember_state(&self, state: Option<DaemonState>) {
        *lock(&self.last_state) = state;
    }

    pub fn last_state(&self) -> Option<DaemonState> {
        *lock(&self.last_state)
    }

    /// Stop the daemon when this window started it, and only then
    ///
    /// A window that lost the start race has no daemon of its own: the one on the socket
    /// belongs to another client, and its Quit is not this window's to decide
    pub fn stop_owned_daemon(&self) {
        {
            let mut daemon = lock(&self.daemon);
            if !daemon.is_ours() {
                return;
            }
        }
        ask_owned_daemon_to_stop(self);
        lock(&self.daemon).stop();
    }
}

// --- Daemon state ---

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub daemon_running: bool,
    /// This GUI started the daemon: only Quit stops such a daemon
    pub daemon_ours: bool,
    pub state: Option<DaemonState>,
    /// A file transcription holds the recognizer while `state` stays `idle`
    pub job: Option<WorkerJob>,
    pub style: Option<String>,
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
        job: None,
        style: None,
        message: Some(err.message),
        hint: err.hint,
        download,
        exit_reason,
    }
}

/// Parse the `status` response: the daemon may not give all fields yet,
/// and a missing field is no error
pub fn parse_status(
    value: &serde_json::Value,
) -> (Option<DaemonState>, Option<String>, Option<WorkerJob>) {
    let state = value
        .get("state")
        .and_then(|v| serde_json::from_value::<DaemonState>(v.clone()).ok());
    let style = value
        .get("style")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // A daemon of an older build has no job field: no job, not an error
    let job = value
        .get("job")
        .and_then(|v| serde_json::from_value::<WorkerJob>(v.clone()).ok());
    (state, style, job)
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

/// How long a recording or a file may wait for a daemon that the window starts itself
///
/// The daemon loads the recognizer before it opens the socket, and on a clean machine it also
/// downloads the weights: a start is a minute on a fast link and much longer on a slow one. The
/// wait is finite because the window must answer, and the error names the likely reason
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(180);

/// How long a replaced daemon has to exit on its own before it is killed
///
/// The stop of a daemon that needs a new process waits for the reply to `Shutdown`: an exit that
/// finishes the journal keeps the phrase that the person just spoke
const DAEMON_STOP_WAIT: Duration = Duration::from_secs(5);

/// The systemd user service that the Nix module declares for the daemon
const DAEMON_SERVICE: &str = "molva-daemon";

/// What the user runs when the daemon belongs to the user manager
const SYSTEMD_START_HINT: &str = "systemctl --user start molva-daemon";

/// The phrase that separates a process that has to be replaced from a failed pipeline
///
/// The daemon answers a section that its process cannot take with `RestartRequired`, so the
/// decision rests on the code and never on the wording of the message
/// Start the daemon when nobody answers on the socket, and wait until it does
///
/// The microphone and the recognizer belong to the daemon, so a recording cannot start without
/// one. The start names the settings file and the socket of this window explicitly: a daemon
/// that read another file or listened elsewhere would be a second owner of the same model
pub(crate) fn ensure_daemon(state: &AppState) -> Result<(), CommandError> {
    if ipc::is_running() {
        return Ok(());
    }
    // A daemon of this window that has not opened the socket yet is the one the wait below is
    // for: a service on the machine does not turn that start into someone else's
    let ours = {
        let mut daemon = lock(&state.daemon);
        daemon.is_ours()
    };
    if !ours {
        start_absent_daemon(daemon_service_declared(), &mut || {
            let socket = ipc::socket_path().map_err(CommandError::from)?;
            let mut daemon = lock(&state.daemon);
            daemon
                .start(state.config_path(), &socket)
                .map_err(CommandError::from)
        })?;
    }
    // The wait happens without the handle: a status poll and a Quit must not stand behind it
    sidecar::wait_ready(DAEMON_READY_TIMEOUT, &ipc::is_running, &mut || {
        lock(&state.daemon).failure()
    })
    .map_err(CommandError::from)
}

/// Bring a daemon up when nobody answers on the socket, or name who owns the one that is missing
///
/// A declared unit means the user manager owns the daemon: a process that the window starts
/// beside it competes with the service, and a stop of that process leaves the service down. The
/// start arrives as an argument, so a test covers this decision without systemd and without
/// spawning a process
fn start_absent_daemon(
    service_declared: bool,
    start_sidecar: &mut dyn FnMut() -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    if service_declared {
        return Err(CommandError::new(
            "daemon_unavailable",
            format!(
                "the daemon does not answer and the {DAEMON_SERVICE} service is declared on this \
                 machine"
            ),
        )
        .with_hint(Some(SYSTEMD_START_HINT.into())));
    }
    start_sidecar()
}

/// Whether the user manager of this machine declares the daemon service
#[cfg(target_os = "linux")]
fn daemon_service_declared() -> bool {
    let answer = std::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "LoadState", DAEMON_SERVICE])
        .output();
    match answer {
        Ok(answer) => unit_is_loaded(&String::from_utf8_lossy(&answer.stdout)),
        // A machine without systemctl has no unit behind the daemon: the window is the only owner
        // left there, and the sidecar is how it gets one
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
const fn daemon_service_declared() -> bool {
    false
}

/// Whether a `LoadState=` answer of `systemctl show` names a unit that the manager loaded
///
/// `loaded` is the only state that gives the daemon an owner: `not-found` is no unit, and
/// `masked` is a service that its owner turned off on purpose
fn unit_is_loaded(answer: &str) -> bool {
    answer.lines().any(|line| line.trim() == "LoadState=loaded")
}

/// Send a command that needs a live daemon, starting it when nobody answers
///
/// The wait for a start is long, and a command that blocks the main thread would freeze the
/// window: the whole sequence runs on a thread of the blocking pool
async fn with_running_daemon<R: Runtime>(
    app: AppHandle<R>,
    cmd: Command,
) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let config = state.config();
        let url = molva_core::infra::llm::openai_compat::configured_base_url(&config.llm);
        let cloud_text = config.llm.enabled
            && config.privacy.send_to_llm
            && !molva_core::infra::llm::openai_compat::is_loopback_url(url);
        let cloud_speech = config.stt.engine == "remote-openai";
        if (cloud_text || cloud_speech) && !config.privacy.cloud_consent {
            return Err(CommandError::new("consent", i18n::t("error.cloudConsent")));
        }
        ensure_daemon(&state)?;
        ipc::request(cmd).map_err(CommandError::from)?;
        Ok(())
    })
    .await
    .map_err(|e| {
        CommandError::new(
            "internal",
            format!("the daemon request was interrupted: {e}"),
        )
    })?
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<Status, CommandError> {
    // The lock is taken and released before the wait: it must not be held across `await`
    let daemon_ours = {
        let mut daemon = lock(&state.daemon);
        daemon.is_ours()
    };
    match ask_daemon(Command::Status).await {
        Ok(value) => {
            let (daemon_state, style, job) = parse_status(&value);
            state.remember_state(daemon_state);
            Ok(Status {
                daemon_running: true,
                daemon_ours,
                state: daemon_state.or(Some(DaemonState::Idle)),
                job,
                style,
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
pub async fn record_start<R: Runtime>(
    app: AppHandle<R>,
    mode: Option<String>,
    style: Option<String>,
) -> Result<(), CommandError> {
    with_running_daemon(
        app,
        Command::RecordStart {
            mode: mode_from(mode.as_deref()),
            style,
        },
    )
    .await
}

/// Stop asks a daemon that is already recording: a window that starts one here would open the
/// microphone to stop a phrase that does not exist
#[tauri::command]
pub async fn record_stop() -> Result<(), CommandError> {
    tell_daemon(Command::RecordStop).await
}

#[tauri::command]
pub async fn record_toggle<R: Runtime>(
    app: AppHandle<R>,
    mode: Option<String>,
    style: Option<String>,
) -> Result<(), CommandError> {
    with_running_daemon(
        app,
        Command::RecordToggle {
            mode: mode_from(mode.as_deref()),
            style,
        },
    )
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
    // An older daemon does not know `devices.list`; the same process can still enumerate the
    // local microphones, so the settings page never falls back to a single "default" entry
    let devices = match ipc::request(Command::DevicesList) {
        Ok(value) => parse_devices(&value)?,
        Err(_) => {
            list_input_devices().map_err(|err| CommandError::new("audio", err.to_string()))?
        }
    };
    state.set_devices(devices.clone());
    Ok(devices)
}

#[tauri::command]
pub async fn list_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, CommandError> {
    let devices = match ask_daemon(Command::DevicesList).await {
        Ok(value) => parse_devices(&value)?,
        Err(_) => tauri::async_runtime::spawn_blocking(list_input_devices)
            .await
            .map_err(|err| CommandError::new("internal", err.to_string()))?
            .map_err(|err| CommandError::new("audio", err.to_string()))?,
    };
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

/// Start the daemon and wait until it answers
///
/// A daemon that downloads weights or loads a model is not a failure: the window must not
/// show "the daemon exited" while the process is working. The command returns only when the
/// socket answers, or with the real startup error
#[tauri::command]
pub async fn start_daemon<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        ensure_daemon(&state)
    })
    .await
    .map_err(|err| {
        CommandError::new(
            "internal",
            format!("the daemon start was interrupted: {err}"),
        )
    })?
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

/// Ask the daemon to apply the settings that are already written, and keep the code of a refusal
///
/// The daemon answers once its worker has taken the settings, so the save acts on the real
/// outcome instead of a guess about the file. The code is what the save needs and a
/// [`CommandError`] does not carry it
fn request_config_reload() -> Result<(), IpcError> {
    match ipc::request(Command::ConfigReload) {
        Ok(_) => Ok(()),
        Err(IpcClientError::Daemon(err)) => Err(IpcError {
            code: err.code,
            message: err.message,
            hint: err.hint,
        }),
        // No daemon holds the socket: the written file is what the next start reads, so nothing
        // refused the settings and no restart is due
        Err(err) if err.is_unavailable() => Ok(()),
        // The daemon answered something this client cannot read: the settings are written, and
        // the user learns that the daemon did not take them
        Err(err) => Err(IpcError {
            code: ErrorCode::Internal,
            message: err.user_message(),
            hint: err.hint(),
        }),
    }
}

/// Whether the daemon refused because its process cannot take the written settings
fn restart_required(err: &IpcError) -> bool {
    err.code == ErrorCode::RestartRequired
}

/// A refusal about a restart in the form that the interface shows
///
/// The message comes from the daemon, which names the sections its process cannot take. The next
/// step is chosen here, because only this window knows who can replace that process: a declared
/// service is restarted by the service manager, and everywhere else the window starts the daemon
/// itself
fn restart_error(refusal: &IpcError) -> CommandError {
    CommandError::new("restart", refusal.message.clone()).with_hint(Some(owner_restart_hint()))
}

/// The next step for a daemon that this window does not own
fn owner_restart_hint() -> String {
    if daemon_service_declared() {
        i18n::t("restart.service")
    } else {
        i18n::t("restart.window")
    }
}

/// What a save does with the answer of the daemon
#[derive(Debug)]
enum ApplyDecision {
    /// The settings are live: the daemon applied them, or no daemon needs them yet
    Live,
    /// The settings need a new process, and this window started the daemon to replace
    RestartOwned(IpcError),
    /// The settings need a new process that belongs to systemd or to another client
    AskOwner(CommandError),
    /// The daemon refused for a reason that no restart fixes
    Refused(CommandError),
}

/// Decide by the answer of the daemon and by the owner of the process, never by the file
///
/// A daemon that another client or systemd started is not this window's to stop: the user gets
/// the daemon's own words and the command that its owner runs
fn apply_decision(daemon_ours: bool, reload: Result<(), IpcError>) -> ApplyDecision {
    let Err(refusal) = reload else {
        return ApplyDecision::Live;
    };
    if !restart_required(&refusal) {
        return ApplyDecision::Refused(
            CommandError::new("daemon", refusal.message).with_hint(refusal.hint),
        );
    }
    if daemon_ours {
        return ApplyDecision::RestartOwned(refusal);
    }
    ApplyDecision::AskOwner(restart_error(&refusal))
}

/// Apply the written settings to the daemon and act on its answer
///
/// `restart` is the effect that replaces the process of this window: it is an argument so a test
/// can prove that a daemon of another owner is never stopped and that a refusal is never dropped
fn apply_to_daemon(
    daemon_ours: bool,
    reload: Result<(), IpcError>,
    restart: &mut dyn FnMut(&IpcError) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    match apply_decision(daemon_ours, reload) {
        ApplyDecision::Live => Ok(()),
        ApplyDecision::RestartOwned(refusal) => restart(&refusal),
        ApplyDecision::AskOwner(refusal) | ApplyDecision::Refused(refusal) => Err(refusal),
    }
}

#[tauri::command]
pub fn get_config_path(state: State<'_, AppState>) -> String {
    state.config_path().display().to_string()
}

/// Where a settings edit lands when the declarative file is a store symlink
#[tauri::command]
pub fn get_runtime_config_path(state: State<'_, AppState>) -> Option<String> {
    Config::runtime_path(state.config_path())
        .ok()
        .flatten()
        .map(|path| path.display().to_string())
}

#[tauri::command]
pub fn is_first_run(state: State<'_, AppState>) -> bool {
    state.first_run()
}

#[tauri::command]
pub fn config_managed(state: State<'_, AppState>) -> bool {
    state.managed()
}

/// Write the settings and bring the daemon and the compositor binds in step with them
///
/// The window, the tray and the pause switch use this one path. The daemon is asked first: a
/// section that its running process cannot take answers with `restart required`, and that answer
/// decides whether a new process is needed. A change of the binds rewrites the Hyprland file
pub(crate) fn apply_config(state: &AppState, config: Config) -> Result<(), CommandError> {
    validate_config(&config)?;
    let hotkeys_changed = state.config().hotkeys != config.hotkeys;
    state.replace_config(config)?;
    let daemon_ours = {
        let mut daemon = lock(&state.daemon);
        daemon.is_ours()
    };
    let reload = request_config_reload();
    let applied = {
        let mut restart = |refusal: &IpcError| restart_daemon(state, refusal);
        apply_to_daemon(daemon_ours, reload, &mut restart)
    };
    // The binds are rewritten even when the daemon needs a restart: the keys live in that file,
    // and the daemon reads them from the settings file that is already written
    if hotkeys_changed {
        refresh_hyprland_binds(&state.config().hotkeys);
    }
    applied
}

/// A command that is not `async` runs on the main thread, and this one writes files and runs
/// `hyprctl reload`: a subprocess that waits for the compositor would stop the window from
/// painting until it answers. The write therefore runs on a thread of the blocking pool, and
/// only the tray, which is GTK, comes back to the main thread
#[tauri::command]
pub async fn save_config(app: AppHandle, config: Config) -> Result<(), CommandError> {
    let write = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = write.state::<AppState>();
        apply_config(&state, config)
    })
    .await
    .map_err(|err| {
        CommandError::new(
            "internal",
            format!("the settings write was interrupted: {err}"),
        )
    })??;

    let tray = app.clone();
    app.run_on_main_thread(move || {
        #[cfg(not(target_os = "linux"))]
        if !tray.state::<AppState>().hotkeys_paused() {
            crate::hotkeys::register(&tray, &tray.state::<AppState>().config().hotkeys);
        }
        crate::tray::refresh(&tray);
    })
    .map_err(|err| CommandError::new("internal", format!("the tray was not refreshed: {err}")))?;
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
///
/// The file reaches the daemon through the same path as a save, and a daemon that cannot take it
/// answers only when its worker is free: the main thread must not wait for that
#[tauri::command]
pub async fn import_config(app: AppHandle, path: String) -> Result<Config, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let path = PathBuf::from(path);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| CommandError::new("config", format!("{}: {e}", path.display())))?;
        let config = Config::from_toml_str(&path, &text)?;
        apply_config(&state, config.clone())?;
        Ok(config)
    })
    .await
    .map_err(|err| {
        CommandError::new(
            "internal",
            format!("the settings import was interrupted: {err}"),
        )
    })?
}

/// The same path as a save for the defaults, including the daemon that has to read them
#[tauri::command]
pub async fn reset_config(app: AppHandle) -> Result<Config, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let config = Config::default();
        apply_config(&state, config.clone())?;
        Ok(config)
    })
    .await
    .map_err(|err| {
        CommandError::new(
            "internal",
            format!("the settings reset was interrupted: {err}"),
        )
    })?
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
pub async fn hyprland_binds() -> Result<crate::hotkeys::HyprlandBinds, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        hyprland_path().map(|path| crate::hotkeys::hyprland_binds(&path))
    })
    .await
    .map_err(|error| CommandError::new("internal", error.to_string()))?
}

/// Write the Hyprland binds from the current hotkeys, then reload Hyprland
///
/// The reload is a subprocess that waits for the compositor, so the write and the reload run
/// off the main thread for the same reason as in `save_config`
#[tauri::command]
pub async fn write_hyprland_binds(
    state: State<'_, AppState>,
) -> Result<crate::hotkeys::HyprlandBinds, CommandError> {
    let hotkeys = state.config().hotkeys;
    tauri::async_runtime::spawn_blocking(move || {
        let binds = crate::hotkeys::write_hyprland_binds(&hyprland_path()?, &hotkeys)
            .map_err(|message| CommandError::new("hyprland", message))?;
        crate::hotkeys::reload_hyprland();
        Ok(binds)
    })
    .await
    .map_err(|err| {
        CommandError::new(
            "internal",
            format!("the binds write was interrupted: {err}"),
        )
    })?
}

/// Stop the daemon that this window started and start it again from the written settings
///
/// A model, an engine or an input device is chosen when the pipeline is built, and a reload of
/// the configuration cannot replace that object. Only a daemon of this window is stopped here: a
/// process of systemd or of another client is its owner's to restart, and its own words and hint
/// name the command that does it
fn restart_daemon(state: &AppState, refusal: &IpcError) -> Result<(), CommandError> {
    let daemon_ours = {
        let mut daemon = lock(&state.daemon);
        daemon.is_ours()
    };
    if !daemon_ours {
        return Err(restart_error(refusal));
    }
    ask_owned_daemon_to_stop(state);
    // The socket answers until the daemon closes it: a start that finds it would leave the old
    // pipeline in place and report the settings as applied
    let deadline = Instant::now() + DAEMON_STOP_WAIT;
    while ipc::is_running() && Instant::now() < deadline {
        std::thread::sleep(READY_STEP);
    }
    lock(&state.daemon).stop();
    ensure_daemon(state).map_err(restart_failure)
}

/// The error of a start that followed a stop: the file is written, and the daemon is not there
fn restart_failure(err: CommandError) -> CommandError {
    CommandError::new("sidecar", format!("saved, restart failed: {}", err.message))
        .with_hint(err.hint)
}

/// Ask the daemon of this handle to stop over the socket
///
/// The reply to `ping` names the pid: a socket that another client's daemon answers is not this
/// window's to stop, and the pid is what tells the two apart
fn ask_owned_daemon_to_stop(state: &AppState) {
    let ours = lock(&state.daemon).pid();
    let answered = ipc::request(Command::Ping)
        .ok()
        .and_then(|value| value.get("pid").and_then(serde_json::Value::as_u64));
    if answered == ours.map(u64::from) {
        // Ask politely first: the daemon closes the microphone and finishes the journal
        let _ = ipc::request(Command::Shutdown);
    }
}

/// After a hotkey change, write the binds file inside a Hyprland session. The file is the
/// single place those keys live, so a save must not leave the old ones behind
fn refresh_hyprland_binds(hotkeys: &HotkeysConfig) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let Some(path) = crate::hotkeys::hyprland_path() else {
        return;
    };
    match crate::hotkeys::write_hyprland_binds(&path, hotkeys) {
        Ok(_) => crate::hotkeys::reload_hyprland(),
        Err(err) => tracing::warn!(%err, "the Hyprland binds file was not rewritten"),
    }
}

#[tauri::command]
pub async fn set_autostart<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<bool, CommandError> {
    // The module of a declarative home writes the entry itself: switching the plugin off would
    // leave that entry in place and the window would start again at the next login, while the
    // switch said off
    refuse_managed_autostart(managed_autostart_file().is_some_and(|path| path.exists()))?;
    tauri::async_runtime::spawn_blocking(move || {
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
    })
    .await
    .map_err(|error| CommandError::new("internal", error.to_string()))?
}

/// Whether the autostart entry belongs to the Nix module rather than to this window
///
/// The page asks before it draws the switch: an entry it cannot remove must not look like one it
/// can
#[tauri::command]
pub fn autostart_managed() -> bool {
    managed_autostart_file().is_some_and(|path| path.exists())
}

/// A window cannot turn off an entry that a configuration wrote
fn refuse_managed_autostart(managed: bool) -> Result<(), CommandError> {
    if !managed {
        return Ok(());
    }
    Err(
        CommandError::new("autostart", i18n::t("error.autostartManaged"))
            .with_hint(Some(i18n::t("settings.autostartManagedHint"))),
    )
}

/// The XDG autostart entry that the Nix module writes, independent of the plugin's own file
fn managed_autostart_file() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart/molvai-gui.desktop"))
}

#[tauri::command]
pub async fn get_autostart<R: Runtime>(app: AppHandle<R>) -> bool {
    if managed_autostart_file().is_some_and(|path| path.exists()) {
        return true;
    }
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

// --- History ---
//
// Reading the journal parses every line of it, sorts it and copies what the filters keep: tens
// of milliseconds on a journal of ten thousand phrases, and seconds on the journal of a person
// who dictates for years. A command without `async` runs on the thread that answers the window,
// so the work of the journal goes to the blocking pool and the window stays answerable

/// How long the window waits for one journal operation before the task counts as lost
const JOURNAL_WORK: &str = "the journal operation was interrupted";

#[tauri::command]
pub async fn history_list(
    state: State<'_, AppState>,
    filter: Option<Filter>,
) -> Result<Vec<Entry>, CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<Entry>, CommandError> {
        let entries = history::load_for(&config)?;
        Ok(history::filter_entries(
            entries,
            &filter.unwrap_or_default(),
        ))
    })
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
}

#[tauri::command]
pub async fn history_apps(state: State<'_, AppState>) -> Result<Vec<String>, CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<String>, CommandError> {
        Ok(history::apps_of(&history::load_for(&config)?))
    })
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
}

#[tauri::command]
pub async fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> Result<Vec<molva_core::app::search::Hit>, CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(
        move || -> Result<Vec<molva_core::app::search::Hit>, CommandError> {
            let entries = history::load_for(&config)?;
            Ok(molva_core::app::search::search(&entries, &query, limit))
        },
    )
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
}

#[tauri::command]
pub async fn history_delete(state: State<'_, AppState>, id: String) -> Result<bool, CommandError> {
    let id = Uuid::parse_str(&id)
        .map_err(|e| CommandError::new("history", format!("invalid id: {e}")))?;
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || -> Result<bool, CommandError> {
        Ok(history::delete(&config, id)?)
    })
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
}

#[tauri::command]
pub async fn history_clear(state: State<'_, AppState>) -> Result<(), CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), CommandError> {
        let path = history::journal_path(&config)?;
        Ok(history::clear(&path)?)
    })
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
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
pub async fn stats_summary(
    state: State<'_, AppState>,
    range_days: Option<u32>,
) -> Result<StatsSummary, CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || -> Result<StatsSummary, CommandError> {
        let entries = history::load_for(&config)?;
        Ok(stats::summary(
            &entries,
            Utc::now(),
            &config.stats,
            range_days.unwrap_or(7),
        ))
    })
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
}

#[tauri::command]
pub async fn stats_export_csv(
    state: State<'_, AppState>,
    range_days: Option<u32>,
) -> Result<String, CommandError> {
    // One export for the window and for `molva stats export`: the table is per phrase, and a
    // second shape built from the day series would be a second answer to the same question
    let _ = range_days;
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, CommandError> {
        let entries = history::load_for(&config)?;
        let dir = history::data_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| CommandError::new("history", e.to_string()))?;
        let path = dir.join(format!("molva-stats-{}.csv", Utc::now().format("%Y-%m-%d")));
        std::fs::write(&path, stats::export_csv(&entries))
            .map_err(|e| CommandError::new("history", e.to_string()))?;
        Ok(path.display().to_string())
    })
    .await
    .map_err(|error| CommandError::new("internal", format!("{JOURNAL_WORK}: {error}")))?
}

/// A stage of one file transcription, as the window shows it
///
/// `percent` stays empty for one file: neither the recognizer nor the daemon reports a share of
/// the work. The queue's share of finished jobs is counted by the window and never travels here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The request waits: the daemon is starting, or it is busy with a phrase or another file
    Starting,
    /// The daemon has the file and works on it
    Engine,
    /// Kept for a client of an older build; the daemon writes the journal before it answers, so
    /// the window never sees this stage
    Finalizing,
}

/// Transcription progress: a phase, the elapsed time and a percentage where one exists
#[derive(Debug, Clone, Serialize)]
pub struct TranscribeProgress {
    pub id: String,
    pub phase: Phase,
    pub percent: Option<u8>,
    /// Epoch milliseconds of the start: the window counts the elapsed time from it
    pub started_at_ms: i64,
    pub elapsed_ms: u64,
}

/// How long a file may wait for a daemon that is busy with a phrase or another file
///
/// The daemon refuses a file while a person records, and the refusal is the right answer: the
/// phrase is gone if it is not delivered. The window keeps asking, because the file can wait
const FILE_BUSY_WAIT: Duration = Duration::from_secs(60);

/// Identifier of the file job that the window waits for, and that `file_cancel` names
///
/// The one-file rule of the daemon is not enough on this side: the window may press cancel while
/// its own job still waits for a daemon that is busy, and the cancel must then reach that job and
/// no other
static RUNNING_FILE_JOB: Mutex<Option<String>> = Mutex::new(None);

/// Clear the identifier of the running job however its transcription ends
struct JobMark;

impl Drop for JobMark {
    fn drop(&mut self) {
        if let Ok(mut slot) = RUNNING_FILE_JOB.lock() {
            *slot = None;
        }
    }
}

/// Transcribe one file through the daemon
///
/// The daemon owns the recognizer: it is the same instance that serves dictation and the MCP
/// server, so a file never loads a second copy of the weights. The daemon is started when none
/// answers; `transcribe` of the CLI stays a separate entry for its offline, batched use
#[tauri::command]
pub async fn transcribe_file<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    path: String,
    chunked: bool,
) -> Result<Vec<molva_core::app::file_result::FileResult>, CommandError> {
    // Transcription is slow, so it runs off the command thread and the interface does not hang
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let started_at_ms = Utc::now().timestamp_millis();
        let started = Instant::now();
        let emit = |phase: Phase| {
            let _ = app.emit(
                "molva://transcribe-progress",
                TranscribeProgress {
                    id: id.clone(),
                    phase,
                    percent: None,
                    started_at_ms,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                },
            );
        };
        emit(Phase::Starting);
        ensure_daemon(&state)?;
        // `~` is a shell shortcut, and no shell sees this path: expand it here
        let resolved = paths::expand_tilde(&path);
        // The daemon decodes the file in its own working directory, so it needs an absolute
        // path; the canonical one is also what its journal entry will name
        let canonical = resolved
            .canonicalize()
            .map_err(|e| CommandError::new("file", format!("{}: {e}", resolved.display())))?;
        let request = FileTranscriptionRequest {
            path: canonical,
            style: None,
            timecodes: true,
            no_llm: false,
            chunked,
            // The journal separates a file from a dictation and from an MCP request
            source: Source::File,
            // The identifier of the queue item: the window cancels the job by this name
            job_id: Some(id.clone()),
        };
        if let Ok(mut slot) = RUNNING_FILE_JOB.lock() {
            *slot = Some(id.clone());
        }
        let _mark = JobMark;
        // The phase follows the answer of the daemon: it works on the file, or it refuses and
        // the window waits, and the stage the user sees is the one that is true now
        match file_request(&request, FILE_BUSY_WAIT, &emit) {
            Ok(report) => {
                let reasons: Vec<String> = report
                    .errors
                    .iter()
                    .map(|failure| format!("{}: {}", failure.file, failure.reason))
                    .collect();
                if !reasons.is_empty() {
                    return Err(CommandError::new("file", reasons.join("; ")));
                }
                Ok(report.results)
            }
            Err(err) => Err(err),
        }
    })
    .await
    .map_err(|e| {
        CommandError::new(
            "file",
            format!("the transcription task was interrupted: {e}"),
        )
    })?
}

/// Stop the file transcription that this window waits for
///
/// The job travels by the identifier of its queue item, so the cancel stops exactly that file: a
/// job that already ended, or the next one in the queue, is left alone with `Ok(false)`
#[tauri::command]
pub fn file_cancel() -> Result<bool, CommandError> {
    let job_id = RUNNING_FILE_JOB.lock().ok().and_then(|slot| slot.clone());
    let Some(job_id) = job_id else {
        // Nothing of this window waits for a file, so there is no job to stop
        return Ok(false);
    };
    let value = ipc::request(Command::FileCancel { job_id })?;
    Ok(value
        .get("cancelled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

/// Ask the daemon for the file until it stops refusing
///
/// The daemon transcribes one file at a time and never beside a phrase. A `Busy` answer is not
/// an error of the request: the person is waiting for a phrase, or another client is
/// transcribing, and the file can wait. Every other failure goes back as it came
fn file_request(
    request: &FileTranscriptionRequest,
    busy_wait: Duration,
    progress: &dyn Fn(Phase),
) -> Result<molva_core::ipc::FileReport, CommandError> {
    let deadline = Instant::now() + busy_wait;
    loop {
        progress(Phase::Engine);
        match ipc::request(Command::FileTranscribe(request.clone())) {
            Ok(value) => {
                return serde_json::from_value(value).map_err(|err| {
                    CommandError::new("daemon", format!("unclear answer from the daemon: {err}"))
                })
            }
            Err(IpcClientError::Daemon(err)) if err.code == ErrorCode::Busy => {
                if Instant::now() >= deadline {
                    return Err(CommandError::from(IpcClientError::Daemon(err)));
                }
                // The daemon refused: the window is waiting again, not transcribing
                progress(Phase::Starting);
                std::thread::sleep(READY_STEP);
            }
            Err(err) => return Err(CommandError::from(err)),
        }
    }
}

/// A file that the window may play: the loopback URL and the media kind
#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub url: String,
    pub kind: MediaKind,
}

/// The loudness of a recording over time, as the window draws it
#[derive(Debug, Clone, Serialize)]
pub struct Waveform {
    /// One byte per bucket, loudest at 255
    pub peaks: Vec<u8>,
    pub duration_ms: u64,
}

/// Buckets in the picture: enough for a wide window, so the bars stay thin on a big screen
const WAVEFORM_BUCKETS: usize = 1200;

/// The peaks of one file, decoded outside the webview
///
/// Decoding through Web Audio is what made the window hang on a long recording: the webview holds
/// the whole stream as a float buffer of its own. Here the decode happens once, in this process,
/// on a thread of its own, and only the picture crosses to the interface: a synchronous command
/// would block the thread that answers the window and freeze it for the length of the decode.
#[tauri::command]
pub async fn waveform(path: String) -> Result<Waveform, CommandError> {
    tauri::async_runtime::spawn_blocking(move || waveform_of(&path))
        .await
        .map_err(|err| {
            CommandError::new("internal", format!("the waveform was interrupted: {err}"))
        })?
}

/// The picture of one file, computed on a worker thread
fn waveform_of(path: &str) -> Result<Waveform, CommandError> {
    let resolved = paths::expand_tilde(path);
    let picture = molva_core::infra::audio::peaks_of_file(&resolved, WAVEFORM_BUCKETS)
        .map_err(|err| CommandError::new("file", err.to_string()))?;
    Ok(Waveform {
        peaks: picture.peaks,
        duration_ms: picture.duration_ms,
    })
}

/// Hand one file to the loopback media server and return the URL that plays it
///
/// The server listens on 127.0.0.1 only, needs the token it was given, and serves only the files
/// that this command handed it. A platform that could not stream an `asset://` URL to a media
/// element gets a plain `http://` one, which is what a player's pipeline accepts.
#[tauri::command]
pub fn prepare_media(state: State<'_, AppState>, path: String) -> Result<MediaInfo, CommandError> {
    let resolved = paths::expand_tilde(&path);
    let canonical = resolved
        .canonicalize()
        .map_err(|e| CommandError::new("file", format!("{}: {e}", resolved.display())))?;
    if !canonical.is_file() {
        return Err(CommandError::new(
            "file",
            format!("{}: not a file", canonical.display()),
        ));
    }
    let kind = paths::media_kind(&canonical);
    let guard = state.media_server()?;
    let server = guard
        .as_ref()
        .ok_or_else(|| CommandError::new("file", "the media server did not start"))?;
    let url = server.allow(&canonical);
    tracing::debug!(%url, file = %canonical.display(), "media handed to the player");
    Ok(MediaInfo { url, kind })
}

/// Entries that complete the typed path: directories first, directories with a trailing slash
///
/// An unreadable directory or a path without a separator gives an empty list, so typing never
/// fails and never blocks.
#[tauri::command]
pub fn path_suggestions(input: String, limit: Option<u32>) -> Vec<PathSuggestion> {
    let home = paths::home_dir();
    paths::suggest_paths_with(&input, home.as_deref(), limit.unwrap_or(10) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::app::secrets::MemoryStore;

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
    fn status_without_fields_is_not_an_error() {
        let (state, style, job) = parse_status(&serde_json::json!({}));
        assert_eq!(state, None);
        assert_eq!(style, None);
        assert_eq!(job, None);
    }

    #[test]
    fn status_reports_state_and_style() {
        let (state, style, job) =
            parse_status(&serde_json::json!({"state": "transcribing", "style": "formal"}));
        assert_eq!(state, Some(DaemonState::Transcribing));
        assert_eq!(style.as_deref(), Some("formal"));
        assert_eq!(job, None);
    }

    #[test]
    fn status_reports_a_file_job_beside_an_idle_state() {
        // The worker may transcribe a file while the state machine is idle: the window must be
        // able to say so instead of "Ready"
        let (state, _, job) =
            parse_status(&serde_json::json!({"state": "idle", "job": "file", "pid": 7}));
        assert_eq!(state, Some(DaemonState::Idle));
        assert_eq!(job, Some(WorkerJob::File));
        // An unknown job of a newer daemon is no job, not a broken status
        let (_, _, unknown) =
            parse_status(&serde_json::json!({"state": "idle", "job": "rendering"}));
        assert_eq!(unknown, None);
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
            models::partial_path(directory.path(), &info.file_name),
            vec![0u8; 1024],
        )
        .unwrap();

        let status = unreachable_status(&config, false, None, CommandError::new("x", "no"));
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
        let status = unreachable_status(&config, false, None, CommandError::new("x", "no"));
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

    #[test]
    fn the_model_list_is_the_catalog_with_the_engine_of_each_model() {
        let options = stt_model_options(&Config::default());
        let gigaam = options
            .iter()
            .find(|option| option.name == "gigaam-v3-e2e-rnnt-q4_k_m")
            .expect("the default model is in the catalog");
        assert_eq!(gigaam.engine, "transcribe-cpp");
        assert_eq!(gigaam.language.as_deref(), Some("ru"));
        for option in &options {
            let info = models::find(&option.name).expect("every option comes from the catalog");
            assert_eq!(option.engine, info.engine, "{}", option.name);
        }
    }

    #[test]
    fn the_config_offers_the_two_local_engines_behind_the_catalog() {
        let engines: Vec<String> = stt_model_options(&Config::default())
            .into_iter()
            .map(|option| option.engine)
            .collect();
        assert!(engines.iter().any(|engine| engine == "whisper-cpp"));
        assert!(engines.iter().any(|engine| engine == "transcribe-cpp"));
    }

    #[test]
    fn remote_recognition_is_available_now_that_the_core_builds_the_engine() {
        // The factory answers RemoteConfig for a missing key or an empty address and never
        // NotAvailable, so the settings page must stop saying "unavailable". The `none` source
        // keeps the OS key store out of this test; it does not decide the verdict
        let mut config = Config::default();
        config.stt.remote.api_key_source = "none".into();
        assert!(remote_stt_available(&config));
    }

    #[test]
    fn an_entry_written_by_a_configuration_cannot_be_switched_off_by_the_window() {
        // The switch would answer "off" while the module's file keeps the window in the session
        let refused = refuse_managed_autostart(true).unwrap_err();
        assert_eq!(refused.kind, "autostart");
        assert!(
            refused.hint.is_some(),
            "the refusal has to name what owns the entry"
        );
        assert!(refuse_managed_autostart(false).is_ok());
    }

    #[test]
    fn a_stored_key_is_reported_present_and_never_echoed() {
        let store = MemoryStore::new();
        let status = store_key(&store, "openai", "sk-secret-value-1234").unwrap();
        assert!(status.present);
        assert_eq!(status.entry, "llm-api-key-openai");
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("sk-secret-value-1234"), "{json}");
        assert!(key_status(&store, "openai").present);
    }

    #[test]
    fn removing_the_key_empties_the_entry() {
        let store = MemoryStore::new();
        store_key(&store, "openrouter", "sk-value").unwrap();
        let status = remove_key(&store, "openrouter").unwrap();
        assert!(!status.present);
        assert!(!key_status(&store, "openrouter").present);
    }

    #[test]
    fn an_empty_key_is_refused_before_the_store_is_touched() {
        let store = MemoryStore::new();
        assert!(store_key(&store, "openai", "   ").is_err());
        assert!(!key_status(&store, "openai").present);
    }

    #[test]
    fn a_refusing_store_is_named_and_not_reported_as_a_missing_key() {
        let store = MemoryStore::failing("no key store on this machine");
        let status = key_status(&store, "openai");
        assert!(!status.present);
        assert!(status.detail.unwrap().contains("no key store"));
    }

    #[test]
    fn the_plain_key_source_and_the_data_collection_choice_are_accepted() {
        let mut config = Config::default();
        config.llm.api_key_source = "plain".into();
        config.llm.api_key_plain = "sk-in-the-file".into();
        config.llm.data_collection = "deny".into();
        assert!(validate_config(&config).is_ok());
        config.llm.data_collection = "sometimes".into();
        assert_eq!(
            validate_config(&config).unwrap_err().field,
            "llm.data_collection"
        );
    }

    #[test]
    fn a_key_source_outside_the_core_list_is_rejected() {
        let mut config = Config::default();
        config.llm.api_key_source = "carrier-pigeon".into();
        assert_eq!(
            validate_config(&config).unwrap_err().field,
            "llm.api_key_source"
        );
    }

    #[test]
    fn a_captured_hotkey_is_checked_by_the_core_parser() {
        assert_eq!(
            check_hotkey("Ctrl+Alt+Z".into()).unwrap().value,
            "Ctrl+Alt+Z"
        );
        assert!(check_hotkey("F9".into()).is_ok());
        assert!(check_hotkey("Ctrl+Telepathy".into()).is_err());
        assert!(check_hotkey(String::new()).is_err());
    }

    #[test]
    fn a_duplicated_custom_style_is_rejected_by_the_core_rules() {
        let mut config = Config::default();
        config.style.custom = vec![
            molva_core::config::CustomStyle {
                id: "polite".into(),
                name: "Polite".into(),
                uses_llm: true,
                system_prompt: "Be polite".into(),
                llm_min_words: 3,
            },
            molva_core::config::CustomStyle {
                id: "polite".into(),
                name: "Again".into(),
                uses_llm: false,
                system_prompt: String::new(),
                llm_min_words: 3,
            },
        ];
        assert_eq!(
            validate_config(&config).unwrap_err().field,
            "style.custom.1.id"
        );
    }

    #[test]
    fn a_style_without_a_prompt_is_rejected_when_it_calls_the_model() {
        let mut config = Config::default();
        config.style.custom = vec![molva_core::config::CustomStyle {
            id: "polite".into(),
            name: "Polite".into(),
            uses_llm: true,
            system_prompt: "  ".into(),
            llm_min_words: 3,
        }];
        assert_eq!(
            validate_config(&config).unwrap_err().field,
            "style.custom.0.system_prompt"
        );
    }

    /// The window asks for completions on every pause in typing, including for a path that is
    /// not there: an unreadable directory must leave the field empty, not raise an error
    #[test]
    fn the_path_suggestions_of_an_unreadable_directory_are_empty() {
        let suggestions = path_suggestions("/definitely-not-here-3f9a/".to_string(), Some(5));
        assert!(suggestions.is_empty(), "{suggestions:?}");
    }

    #[test]
    fn the_path_suggestions_put_directories_first_and_keep_them_open() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::create_dir(directory.path().join("folder")).expect("a directory");
        std::fs::write(directory.path().join("file.wav"), b"").expect("a file");
        let input = format!("{}/", directory.path().display());
        let suggestions = path_suggestions(input, Some(10));
        let folder = suggestions
            .iter()
            .find(|item| item.name == "folder")
            .expect("the directory is offered");
        assert!(folder.is_dir);
        assert!(folder.path.ends_with('/'), "{}", folder.path);
        let file = suggestions
            .iter()
            .find(|item| item.name == "file.wav")
            .expect("the file is offered");
        assert!(!file.is_dir);
    }

    #[test]
    fn more_than_ten_custom_styles_are_rejected() {
        let style = |index: usize| molva_core::config::CustomStyle {
            id: format!("style-{index}"),
            name: format!("Style {index}"),
            uses_llm: false,
            system_prompt: String::new(),
            llm_min_words: 3,
        };
        let mut config = Config::default();
        config.style.custom = (0..11).map(style).collect();
        assert_eq!(validate_config(&config).unwrap_err().field, "style.custom");
    }

    /// The refusal of a daemon whose process cannot take the written sections
    fn restart_refusal() -> IpcError {
        IpcError {
            code: ErrorCode::RestartRequired,
            message: "settings saved but not applied: restart required: stt, audio".into(),
            hint: Some("restart the daemon: systemctl --user restart molva-daemon".into()),
        }
    }

    #[test]
    fn only_the_daemon_this_window_started_is_restarted() {
        assert!(
            matches!(
                apply_decision(true, Err(restart_refusal())),
                ApplyDecision::RestartOwned(_)
            ),
            "a daemon of this window was not restarted: {:?}",
            apply_decision(true, Err(restart_refusal()))
        );
        match apply_decision(false, Err(restart_refusal())) {
            ApplyDecision::AskOwner(err) => {
                assert_eq!(err.kind, "restart");
                assert_eq!(err.message, restart_refusal().message);
                assert_eq!(err.hint, Some(owner_restart_hint()));
            }
            other => panic!("a daemon of another owner was not left to it: {other:?}"),
        }
    }

    #[test]
    fn a_daemon_of_another_owner_is_not_stopped() {
        // The restart effect is the one that sends `Shutdown` and starts a second process
        let mut restarts = 0;
        let err = apply_to_daemon(false, Err(restart_refusal()), &mut |_refusal| {
            restarts += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            restarts, 0,
            "the window stopped a daemon that is not its own"
        );
        assert_eq!(err.kind, "restart");
        assert_eq!(err.message, restart_refusal().message);
        assert_eq!(err.hint, Some(owner_restart_hint()));
    }

    #[test]
    fn an_owned_daemon_gets_a_new_process_when_the_sections_need_one() {
        let mut restarts = 0;
        let result = apply_to_daemon(true, Err(restart_refusal()), &mut |_refusal| {
            restarts += 1;
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(restarts, 1);
    }

    #[test]
    fn a_live_reload_leaves_the_daemon_alone() {
        let mut restarts = 0;
        let result = apply_to_daemon(false, Ok(()), &mut |_refusal| {
            restarts += 1;
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(restarts, 0);
        assert!(matches!(apply_decision(true, Ok(())), ApplyDecision::Live));
        assert!(matches!(apply_decision(false, Ok(())), ApplyDecision::Live));
    }

    #[test]
    fn an_error_of_another_kind_is_not_read_as_a_restart() {
        for code in [ErrorCode::Internal, ErrorCode::Busy, ErrorCode::SttFailed] {
            let err = IpcError {
                code,
                message: "settings saved but not applied: the pipeline did not take them".into(),
                hint: None,
            };
            let mut restarts = 0;
            let reported = apply_to_daemon(true, Err(err), &mut |_refusal| {
                restarts += 1;
                Ok(())
            })
            .unwrap_err();
            assert_eq!(restarts, 0, "{code:?} was answered with a restart");
            assert_eq!(reported.kind, "daemon", "{code:?}");
        }
    }

    #[test]
    fn a_refused_settings_file_is_not_read_as_a_restart() {
        // `BadRequest` covers every refusal of a written file: the phrase is what tells a
        // process that has to be replaced from a file that the pipeline could not load
        let load_failure = IpcError {
            code: ErrorCode::BadRequest,
            message: "settings not reloaded: unknown field `sttt`".into(),
            hint: Some("check the file: molva config validate".into()),
        };
        let mut restarts = 0;
        let reported = apply_to_daemon(true, Err(load_failure), &mut |_refusal| {
            restarts += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(restarts, 0);
        assert_eq!(reported.kind, "daemon");
        assert_eq!(
            reported.message,
            "settings not reloaded: unknown field `sttt`"
        );
        assert_eq!(
            reported.hint.as_deref(),
            Some("check the file: molva config validate")
        );
    }

    #[test]
    fn a_failed_restart_says_that_the_settings_are_saved() {
        let reported = apply_to_daemon(true, Err(restart_refusal()), &mut |_refusal| {
            Err(restart_failure(CommandError::new(
                "sidecar",
                "molva executable not found: put it next to the GUI or in PATH",
            )))
        })
        .unwrap_err();
        assert_eq!(reported.kind, "sidecar");
        assert!(
            reported.message.starts_with("saved, restart failed: "),
            "{}",
            reported.message
        );
        assert!(
            reported.message.contains("molva executable not found"),
            "{}",
            reported.message
        );
    }

    #[test]
    fn a_declared_service_keeps_the_window_from_starting_a_second_daemon() {
        let mut starts = 0;
        let err = start_absent_daemon(true, &mut || {
            starts += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(starts, 0, "a sidecar started beside the service");
        assert_eq!(err.kind, "daemon_unavailable");
        assert_eq!(
            err.hint.as_deref(),
            Some("systemctl --user start molva-daemon")
        );
    }

    #[test]
    fn without_a_service_the_window_starts_the_sidecar() {
        let mut starts = 0;
        let result = start_absent_daemon(false, &mut || {
            starts += 1;
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(starts, 1);
    }

    #[test]
    fn only_a_loaded_unit_counts_as_a_declared_service() {
        assert!(unit_is_loaded("LoadState=loaded\n"));
        // A unit the manager does not know, and one its owner turned off on purpose
        assert!(!unit_is_loaded("LoadState=not-found\n"));
        assert!(!unit_is_loaded("LoadState=masked\n"));
        // No systemctl, or an answer this client cannot read: no unit owns the daemon
        assert!(!unit_is_loaded(""));
        assert!(!unit_is_loaded(
            "Failed to connect to bus: No such file or directory"
        ));
    }
}
