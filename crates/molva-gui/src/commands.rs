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
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use molva_core::app::engine::{self, build_stt_with, EngineChoice, EngineError};
use molva_core::app::hotkeys::HotkeySpec;
use molva_core::app::models::{self, DownloadProgress};
use molva_core::app::secrets::{self, SecretError, SecretStore};
use molva_core::config::{invalid_provider_slug, ConfigError};
use molva_core::domain::entry::{Mode, Source};
use molva_core::domain::llm::{ChatRequest, LlmClient, LlmError};
use molva_core::domain::{DeviceInfo, Entry, OutputMode};
use molva_core::infra::audio::list_input_devices;
use molva_core::infra::llm::OpenAiCompatClient;
use molva_core::ipc::{Command, DaemonState, FileTranscriptionRequest, WorkerJob};
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
/// The question is put to the engine factory rather than answered from a list: the day the
/// core builds a remote client, the settings page stops saying "unavailable" by itself
pub fn remote_stt_available(config: &Config) -> bool {
    let mut probe = config.clone();
    probe.stt.engine = "remote-openai".into();
    !matches!(
        build_stt_with(&probe, &EngineChoice::default()),
        Err(EngineError::NotAvailable(_))
    )
}

#[tauri::command]
pub async fn stt_remote_available(state: State<'_, AppState>) -> Result<bool, CommandError> {
    let config = state.config();
    tauri::async_runtime::spawn_blocking(move || remote_stt_available(&config))
        .await
        .map_err(|error| CommandError::new("internal", error.to_string()))
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

/// Prompt of the model test
///
/// A constant, so the request carries nothing the person dictated: the test costs one short
/// answer and tells a key, a URL and a model apart from a broken setting
pub const LLM_PROBE_PROMPT: &str = "Reply with the single word: ok";

/// System prompt of the test: it needs no rules, but some providers reject an empty message
const LLM_PROBE_SYSTEM: &str = "This is a connectivity check. Answer in one word.";

/// How many characters of the answer the report keeps
const LLM_PROBE_ANSWER: usize = 200;

/// How many tokens the test may spend on its answer
const LLM_PROBE_MAX_TOKENS: u32 = 16;

/// What the model test found, as the settings page shows it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmProbeReport {
    /// The provider that answered, by its identifier: `ollama`, `openrouter`…
    pub provider: String,
    /// The model of the draft
    pub model: String,
    /// The answer came from this machine
    pub is_local: bool,
    /// From the first request to the answer, retries included
    pub latency_ms: u64,
    /// How many requests went out: a figure above one means the provider failed and answered
    pub attempts: u32,
    /// The model text, cut to a short line
    pub answer: String,
}

/// One line of the recognition test
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SttProbeCheck {
    /// `engine` | `remote` | `model` | `cli`, so the page can translate the title
    pub id: String,
    pub ok: bool,
    /// What the check found: an error text, or the path it used. Empty when there is nothing
    /// to add to a successful check
    pub detail: String,
}

/// What the recognition test found
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SttProbeReport {
    /// The engine that the draft resolves to, named by the factory itself
    pub engine: String,
    pub model: String,
    /// Every check passed
    pub ok: bool,
    pub checks: Vec<SttProbeCheck>,
}

/// The request of the model test: one short answer, no temperature
fn probe_request(cfg: &molva_core::config::LlmConfig) -> ChatRequest {
    ChatRequest {
        model: cfg.model.clone(),
        system: LLM_PROBE_SYSTEM.into(),
        user: LLM_PROBE_PROMPT.into(),
        temperature: 0.0,
        max_tokens: LLM_PROBE_MAX_TOKENS,
    }
}

/// One request under the retry policy of the pipeline: the answer, the attempt it came on and
/// the time it took
///
/// The rules repeat `Pipeline::complete_with_retries`: `max_retries` more requests after the
/// first, an authorization error is not retried because a retry does not fix the key, and an
/// empty answer counts as a failure. The loop cannot be shared with the pipeline: there it is a
/// method of the phrase pipeline, and a test with no phrase cannot reach it
fn probe_complete(
    client: &dyn LlmClient,
    request: &ChatRequest,
    max_retries: u32,
) -> Result<(String, u32, u64), LlmError> {
    let attempts = max_retries.saturating_add(1);
    let started = Instant::now();
    let mut last = LlmError::Disabled;
    for attempt in 1..=attempts {
        match client.complete(request) {
            Ok(response) => {
                if response.text.trim().is_empty() {
                    last = LlmError::BadResponse("the model returned empty text".into());
                    continue;
                }
                return Ok((response.text, attempt, started.elapsed().as_millis() as u64));
            }
            Err(LlmError::Auth) => return Err(LlmError::Auth),
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// The key as the daemon would read it, with a bound on the OS key store
///
/// A locked keyring may wait for an unlock prompt that nothing shows, and the button must answer
/// instead of spinning forever. A store that stays silent leaves the environment variable, which
/// `api_key_with` reads on its own
fn probe_api_key(cfg: &molva_core::config::LlmConfig) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    let probe = cfg.clone();
    std::thread::spawn(move || {
        let _ = tx.send(secrets::api_key(&probe));
    });
    rx.recv_timeout(KEYRING_TIMEOUT)
        .unwrap_or_else(|_| secrets::api_key_with(cfg, |name| std::env::var(name).ok()))
}

/// A model failure as the settings page shows it, with the next step of each kind
fn llm_command_error(error: LlmError) -> CommandError {
    let hint = match &error {
        LlmError::Auth => Some(i18n::t("settings.llm.probeAuthHint")),
        LlmError::Timeout(seconds) => Some(i18n::tv(
            "settings.llm.probeTimeoutHint",
            &[("seconds", &seconds.to_string())],
        )),
        LlmError::Unavailable(_) => Some(i18n::t("settings.llm.probeUnavailableHint")),
        LlmError::BadResponse(_) | LlmError::Disabled => None,
    };
    CommandError::new("llm", error.to_string()).with_hint(hint)
}

/// Run the draft model configuration against a harmless prompt
///
/// The draft goes in as it stands in the form: no save, no reload, and the key comes from the
/// source the draft names. A cloud provider needs the same confirmation as a phrase, because the
/// request leaves the machine
pub fn llm_probe(config: &Config) -> Result<LlmProbeReport, CommandError> {
    let cfg = &config.llm;
    if let Some(token) = invalid_provider_slug(&cfg.provider_order) {
        return Err(ValidationError {
            field: "llm.provider_order".into(),
            message: i18n::tv("validation.providerSlug", &[("value", &token)]),
            allowed: Vec::new(),
        }
        .into());
    }
    if cfg.model.trim().is_empty() {
        return Err(CommandError {
            kind: "validation".into(),
            message: i18n::t("validation.noLlmModel"),
            hint: Some(i18n::t("settings.llm.modelHint")),
            field: Some("llm.model".into()),
        });
    }
    if is_cloud_provider(&cfg.provider) && !config.privacy.cloud_consent {
        return Err(CommandError::new("consent", i18n::t("error.cloudConsent"))
            .with_hint(Some(i18n::t("settings.llm.probeConsentHint"))));
    }
    let client =
        OpenAiCompatClient::from_config(cfg, probe_api_key(cfg)).map_err(llm_command_error)?;
    let (answer, attempts, latency_ms) =
        probe_complete(&client, &probe_request(cfg), cfg.max_retries).map_err(llm_command_error)?;
    Ok(LlmProbeReport {
        provider: client.id().to_string(),
        model: cfg.model.trim().to_string(),
        is_local: client.is_local(),
        latency_ms,
        attempts,
        answer: answer.trim().chars().take(LLM_PROBE_ANSWER).collect(),
    })
}

/// Where `Command` would find a program of `PATH`, or `None`
///
/// `PATH` alone: a program is what a recognition engine would execute, not a file beside it
fn program_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

/// The program that the recognition engine will execute, and whether it is there
fn transcribe_cli_check(configured: &str) -> SttProbeCheck {
    let configured = configured.trim();
    let found = if configured.is_empty() {
        program_in_path("transcribe-cli")
    } else {
        let path = paths::expand_tilde(configured);
        path.is_file().then_some(path)
    };
    match found {
        Some(path) => SttProbeCheck {
            id: "cli".into(),
            ok: true,
            detail: path.display().to_string(),
        },
        None => SttProbeCheck {
            id: "cli".into(),
            ok: false,
            detail: i18n::t("settings.sttTest.cliMissing"),
        },
    }
}

/// Check the draft recognition settings without recording anything
///
/// The engine is built and dropped: building checks the engine name and the weights of a local
/// engine, loads no model and opens no device, so the microphone stays untouched. The weights and
/// the `transcribe-cli` binary are named separately, because each one has its own fix
pub fn stt_probe(config: &Config) -> SttProbeReport {
    let model = config.stt.model.trim().to_string();
    let engine = engine::resolve_engine(config, &EngineChoice::default());
    let mut checks: Vec<SttProbeCheck> = Vec::new();
    match build_stt_with(config, &EngineChoice::default()) {
        Ok(built) => checks.push(SttProbeCheck {
            id: "engine".into(),
            ok: true,
            detail: built.id().to_string(),
        }),
        // The factory answers `NotAvailable` for an engine that this build does not carry: the
        // page says that instead of pretending the draft is wrong
        Err(EngineError::NotAvailable(_)) => checks.push(SttProbeCheck {
            id: "remote".into(),
            ok: false,
            detail: i18n::t("settings.sttTest.remoteUnavailable"),
        }),
        Err(error) => checks.push(SttProbeCheck {
            id: "engine".into(),
            ok: false,
            detail: error.to_string(),
        }),
    }
    if engine != "remote-openai" {
        match models::installed_path(config, &model) {
            Ok(path) => checks.push(SttProbeCheck {
                id: "model".into(),
                ok: true,
                detail: path.display().to_string(),
            }),
            Err(error) => checks.push(SttProbeCheck {
                id: "model".into(),
                ok: false,
                detail: error.to_string(),
            }),
        }
        if engine == "transcribe-cpp" {
            checks.push(transcribe_cli_check(&config.stt.transcribe_cli));
        }
    }
    SttProbeReport {
        engine,
        model,
        ok: checks.iter().all(|check| check.ok),
        checks,
    }
}

/// Test the draft model configuration: a short prompt, the real client, the real retry policy
#[tauri::command]
pub async fn test_llm(config: Config) -> Result<LlmProbeReport, CommandError> {
    // The request waits for the network, and the window must not wait with it
    tauri::async_runtime::spawn_blocking(move || llm_probe(&config))
        .await
        .map_err(|error| CommandError::new("internal", error.to_string()))?
}

/// Test the draft recognition settings; nothing is recorded and no device is opened
#[tauri::command]
pub async fn test_stt(config: Config) -> Result<SttProbeReport, CommandError> {
    tauri::async_runtime::spawn_blocking(move || stt_probe(&config))
        .await
        .map_err(|error| CommandError::new("internal", error.to_string()))
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
    /// The config resolves into the Nix store: the settings page is hidden and writes are refused
    managed: bool,
    /// There was no settings file: the interface opens on the Settings tab at once
    first_run: bool,
    daemon: Mutex<Daemon>,
    hotkeys_paused: AtomicBool,
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
            hotkeys_paused: AtomicBool::new(false),
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

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn managed(&self) -> bool {
        self.managed
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

    /// Stop the daemon when this window started it, and only then
    ///
    /// A window that lost the start race has no daemon of its own: the one on the socket
    /// belongs to another client, and its Quit is not this window's to decide. The pid of the
    /// answer tells the two apart
    pub fn stop_owned_daemon(&self) {
        let mut daemon = lock(&self.daemon);
        if !daemon.is_ours() {
            return;
        }
        let ours = daemon.pid();
        let answered = ipc::request(Command::Ping)
            .ok()
            .and_then(|value| value.get("pid").and_then(serde_json::Value::as_u64));
        if answered == ours.map(u64::from) {
            // Ask politely first: the daemon closes the microphone and finishes the journal
            let _ = ipc::request(Command::Shutdown);
        }
        daemon.stop();
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
        job: None,
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

/// Start the daemon when nobody answers on the socket, and wait until it does
///
/// The microphone and the recognizer belong to the daemon, so a recording cannot start without
/// one. The start names the settings file and the socket of this window explicitly: a daemon
/// that read another file or listened elsewhere would be a second owner of the same model
pub(crate) fn ensure_daemon(state: &AppState) -> Result<(), CommandError> {
    if ipc::is_running() {
        return Ok(());
    }
    let socket = ipc::socket_path().map_err(CommandError::from)?;
    {
        let mut daemon = lock(&state.daemon);
        daemon
            .start(state.config_path(), &socket)
            .map_err(CommandError::from)?;
    }
    // The wait happens without the handle: a status poll and a Quit must not stand behind it
    sidecar::wait_ready(DAEMON_READY_TIMEOUT, &ipc::is_running, &mut || {
        lock(&state.daemon).failure()
    })
    .map_err(CommandError::from)
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
        let cloud_text = config.llm.enabled
            && config.privacy.send_to_llm
            && is_cloud_provider(&config.llm.provider);
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
pub fn config_managed(state: State<'_, AppState>) -> bool {
    state.managed()
}

/// A managed config is a store symlink: a write fails at the file system with a message that
/// does not name Nix. This check answers before the write and names the place to change.
fn refuse_managed(state: &AppState) -> Result<(), CommandError> {
    if state.managed() {
        return Err(CommandError::new(
            "config",
            "the settings are managed by Nix; change programs.molvai and run switch",
        ));
    }
    Ok(())
}

/// Write the settings, keep them, and bring the hotkeys in step with them
///
/// A command that is not `async` runs on the main thread, and this one writes files and runs
/// `hyprctl reload`: a subprocess that waits for the compositor would stop the window from
/// painting until it answers. The write therefore runs on a thread of the blocking pool, and
/// only the tray, which is GTK, comes back to the main thread
#[tauri::command]
pub async fn save_config(app: AppHandle, config: Config) -> Result<(), CommandError> {
    let write = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = write.state::<AppState>();
        refuse_managed(&state)?;
        validate_config(&config)?;
        let previous = state.config();
        let hotkeys_changed = previous.hotkeys != config.hotkeys;
        // A recogniser is built from the engine, the model and the audio device. The running
        // daemon holds the old pipeline, so those changes need a fresh process; the rest are
        // applied by `config.reload` without dropping the model
        let restart = previous.stt != config.stt || previous.audio != config.audio;
        state.replace_config(config)?;
        if restart {
            restart_daemon(&state);
        } else {
            notify_config_reload();
        }
        if hotkeys_changed {
            refresh_hyprland_binds(&state.config().hotkeys);
        }
        Ok::<(), CommandError>(())
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
#[tauri::command]
pub fn import_config(state: State<'_, AppState>, path: String) -> Result<Config, CommandError> {
    refuse_managed(&state)?;
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
    refuse_managed(&state)?;
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

/// Stop the daemon and start it again from the settings that were just written
///
/// A model, an engine or an input device is chosen when the pipeline is built. A reload of the
/// configuration cannot replace that object, so keeping the old daemon would leave the warning
/// "the previous model is still in use" true after a save. The wait is bounded: a failed start
/// is reported by the next status request, not by a stuck settings page
fn restart_daemon(state: &AppState) {
    if !ipc::is_running() {
        return;
    }
    // A daemon started by another client is foreign: it still has to stop, otherwise the new
    // process would find its socket and do nothing
    let _ = ipc::request(Command::Shutdown);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ipc::is_running() && Instant::now() < deadline {
        std::thread::sleep(READY_STEP);
    }
    lock(&state.daemon).stop();
    if let Err(err) = ensure_daemon(state) {
        tracing::warn!(error = %err.message, "the daemon was not restarted after a settings change");
    }
}

/// After a hotkey change, write the binds file inside a Hyprland session. The file is the
/// single place those keys live, so a save must not leave the old ones behind
fn refresh_hyprland_binds(hotkeys: &molva_core::config::HotkeysConfig) {
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

#[tauri::command]
pub async fn get_autostart<R: Runtime>(app: AppHandle<R>) -> bool {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().unwrap_or(false)
    })
    .await
    .unwrap_or(false)
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
pub fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> Result<Vec<molva_core::app::search::Hit>, CommandError> {
    let entries = history::load_for(&state.config())?;
    Ok(molva_core::app::search::search(&entries, &query, limit))
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
            // The journal separates a file from a dictation and from an MCP request
            source: Source::File,
        };
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
            Err(IpcClientError::Daemon(err)) if err.code == molva_core::ipc::ErrorCode::Busy => {
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
    fn local_providers_need_no_confirmation_cloud_ones_do() {
        assert!(!is_cloud_provider("ollama"));
        assert!(!is_cloud_provider("lmstudio"));
        assert!(is_cloud_provider("openai"));
        assert!(is_cloud_provider("groq"));
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
    fn remote_recognition_is_unavailable_while_the_core_refuses_the_engine() {
        // The day the core builds a remote client this fails, and the settings page must stop
        // saying "unavailable": the question is put to the engine factory, not to a list
        assert!(!remote_stt_available(&Config::default()));
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
            },
            molva_core::config::CustomStyle {
                id: "polite".into(),
                name: "Again".into(),
                uses_llm: false,
                system_prompt: String::new(),
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
        }];
        assert_eq!(
            validate_config(&config).unwrap_err().field,
            "style.custom.0.system_prompt"
        );
    }

    #[test]
    fn more_than_ten_custom_styles_are_rejected() {
        let style = |index: usize| molva_core::config::CustomStyle {
            id: format!("style-{index}"),
            name: format!("Style {index}"),
            uses_llm: false,
            system_prompt: String::new(),
        };
        let mut config = Config::default();
        config.style.custom = (0..11).map(style).collect();
        assert_eq!(validate_config(&config).unwrap_err().field, "style.custom");
    }

    // --- The test buttons of the settings page ---

    /// A draft with the key from nowhere: the tests never read the OS key store
    fn local_draft(base_url: &str) -> Config {
        let mut config = Config::default();
        config.llm.enabled = true;
        config.llm.provider = "ollama".into();
        config.llm.base_url = base_url.into();
        config.llm.model = "qwen3.5:4b".into();
        config.llm.api_key_source = "none".into();
        config.llm.max_retries = 2;
        config
    }

    #[test]
    fn the_probe_request_carries_no_dictated_text() {
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let request = probe_request(&cfg);
        assert_eq!(request.user, LLM_PROBE_PROMPT);
        assert_eq!(request.model, "qwen3.5:4b");
        assert_eq!(request.max_tokens, LLM_PROBE_MAX_TOKENS);
        assert_eq!(request.temperature, 0.0);
        assert!(!request.user.contains("текст"));
    }

    #[test]
    fn a_probe_reports_the_answer_the_attempt_and_the_time() {
        let client = molva_core::domain::fakes::FakeLlm::echoing("ok");
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let (answer, attempts, _) =
            probe_complete(&client, &probe_request(&cfg), cfg.max_retries).unwrap();
        assert_eq!(answer, "ok");
        assert_eq!(attempts, 1);
        assert_eq!(client.calls(), 1);
    }

    #[test]
    fn a_probe_spends_the_retry_budget_and_never_more() {
        let client = molva_core::domain::fakes::FakeLlm::failing(LlmError::Unavailable(
            "model is loading".into(),
        ));
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let error = probe_complete(&client, &probe_request(&cfg), cfg.max_retries).unwrap_err();
        assert!(matches!(error, LlmError::Unavailable(_)), "{error:?}");
        // max_retries = 2 means one request and two retries
        assert_eq!(client.calls(), 3);
    }

    #[test]
    fn a_rejected_key_is_not_retried() {
        let client = molva_core::domain::fakes::FakeLlm::failing(LlmError::Auth);
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let error = probe_complete(&client, &probe_request(&cfg), cfg.max_retries).unwrap_err();
        assert_eq!(error, LlmError::Auth);
        assert_eq!(client.calls(), 1, "a retry does not fix the key");
    }

    #[test]
    fn an_empty_answer_is_a_failure_and_a_retry() {
        let client = molva_core::domain::fakes::FakeLlm::new(|_| {
            Ok(molva_core::domain::llm::ChatResponse {
                text: "   ".into(),
                prompt_tokens: None,
                completion_tokens: None,
            })
        });
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let error = probe_complete(&client, &probe_request(&cfg), 1).unwrap_err();
        assert!(matches!(error, LlmError::BadResponse(_)), "{error:?}");
        assert_eq!(client.calls(), 2);
    }

    #[test]
    fn the_model_test_of_a_cloud_draft_needs_the_cloud_consent() {
        let mut config = local_draft("https://openrouter.ai/api/v1");
        config.llm.provider = "openrouter".into();
        config.privacy.cloud_consent = false;
        let error = llm_probe(&config).unwrap_err();
        assert_eq!(error.kind, "consent");
        assert!(error.hint.is_some(), "the page must name the next step");
    }

    #[test]
    fn the_model_test_refuses_a_draft_without_a_model() {
        let mut config = local_draft("http://127.0.0.1:9/v1");
        config.llm.model = "  ".into();
        let error = llm_probe(&config).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("llm.model"));
    }

    #[test]
    fn the_model_test_refuses_a_hoster_order_that_is_not_a_list_of_slugs() {
        let mut config = local_draft("https://openrouter.ai/api/v1");
        config.llm.provider = "openrouter".into();
        config.privacy.cloud_consent = true;
        config.llm.provider_order = "groq; together".into();
        let error = llm_probe(&config).unwrap_err();
        assert_eq!(error.kind, "validation");
        assert_eq!(error.field.as_deref(), Some("llm.provider_order"));
    }

    #[test]
    fn a_dead_local_model_becomes_an_error_with_a_next_step() {
        // A closed port on this machine: no outside network is touched
        let config = local_draft("http://127.0.0.1:9/v1");
        let error = llm_probe(&config).unwrap_err();
        assert_eq!(error.kind, "llm", "{error:?}");
        assert!(error.hint.is_some(), "{error:?}");
        assert!(
            error.message.contains("127.0.0.1:9"),
            "the address names the place: {}",
            error.message
        );
    }

    /// A directory with the weights of the default model, written as dummy bytes
    fn models_with_default_weights(directory: &std::path::Path) -> Config {
        let info = models::find("gigaam-v3-e2e-rnnt-q4_k_m").unwrap();
        std::fs::write(directory.join(&info.file_name), "weights").unwrap();
        let mut config = Config::default();
        config.stt.model_path = directory.display().to_string();
        config.stt.engine = "auto".into();
        config.stt.model = "gigaam-v3-e2e-rnnt-q4_k_m".into();
        config
    }

    #[test]
    fn the_recognition_test_names_the_engine_and_the_missing_weights() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stt.model_path = directory.path().display().to_string();
        let report = stt_probe(&config);
        assert_eq!(report.engine, "transcribe-cpp");
        assert!(!report.ok);
        let ids: Vec<&str> = report.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["engine", "model", "cli"]);
        let model = report.checks.iter().find(|c| c.id == "model").unwrap();
        assert!(!model.ok);
        assert!(
            model.detail.contains("molva models pull"),
            "{}",
            model.detail
        );
    }

    #[test]
    fn the_recognition_test_passes_when_the_weights_and_the_cli_are_there() {
        let directory = tempfile::tempdir().unwrap();
        let cli = directory.path().join("transcribe-cli");
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        let mut config = models_with_default_weights(directory.path());
        config.stt.transcribe_cli = cli.display().to_string();
        let report = stt_probe(&config);
        assert!(report.ok, "{report:?}");
        assert_eq!(report.engine, "transcribe-cpp");
        let model = report.checks.iter().find(|c| c.id == "model").unwrap();
        assert!(model.ok);
        let info = models::find("gigaam-v3-e2e-rnnt-q4_k_m").unwrap();
        assert!(model.detail.ends_with(&info.file_name), "{}", model.detail);
        let cli = report.checks.iter().find(|c| c.id == "cli").unwrap();
        assert!(cli.ok);
    }

    #[test]
    fn the_recognition_test_reports_a_transcribe_cli_that_is_not_there() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = models_with_default_weights(directory.path());
        config.stt.transcribe_cli = directory.path().join("no-such-cli").display().to_string();
        let report = stt_probe(&config);
        assert!(!report.ok);
        let cli = report.checks.iter().find(|c| c.id == "cli").unwrap();
        assert!(!cli.ok);
        assert!(!cli.detail.is_empty(), "the page needs a reason");
        // The engine and the weights are still fine: only one piece is missing
        assert!(
            report.checks.iter().filter(|c| c.ok).count() >= 2,
            "{report:?}"
        );
    }

    #[test]
    fn remote_recognition_is_reported_as_not_implemented() {
        let mut config = Config::default();
        config.stt.engine = "remote-openai".into();
        let report = stt_probe(&config);
        assert!(!report.ok);
        assert_eq!(report.engine, "remote-openai");
        assert_eq!(report.checks.len(), 1, "{report:?}");
        let check = &report.checks[0];
        assert_eq!(check.id, "remote");
        assert!(!check.ok);
        assert!(
            check.detail.contains("does not implement"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn a_whisper_draft_needs_no_transcribe_cli() {
        let directory = tempfile::tempdir().unwrap();
        let info = models::find("tiny").unwrap();
        std::fs::write(directory.path().join(&info.file_name), "weights").unwrap();
        let mut config = Config::default();
        config.stt.model_path = directory.path().display().to_string();
        config.stt.engine = "auto".into();
        config.stt.model = "tiny".into();
        let report = stt_probe(&config);
        assert_eq!(report.engine, "whisper-cpp");
        assert!(report.ok, "{report:?}");
        assert!(report.checks.iter().all(|c| c.id != "cli"), "{report:?}");
    }
}
