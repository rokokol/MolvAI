// SPDX-License-Identifier: MIT
// Draft checks for the Settings page, run by the daemon
//
// Both checks take the draft as it stands in the form. Neither writes the settings, opens the
// microphone or records a phrase. They live here, and not in the window, because the daemon owns
// the key source, the engine factory and the environment that the engine needs

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::{engine, models, secrets};
use crate::config::{invalid_provider_slug, Config, LlmConfig};
use crate::domain::llm::{ChatRequest, LlmClient, LlmError};
use crate::infra::llm::OpenAiCompatClient;

/// Prompt of the model test: it carries nothing the person dictated
pub const LLM_PROBE_PROMPT: &str = "Reply with the single word: ok";

/// The program behind the default GigaAM model
pub const TRANSCRIBE_CLI: &str = "transcribe-cli";

const LLM_PROBE_SYSTEM: &str = "This is a connectivity check. Answer in one word.";
const LLM_PROBE_ANSWER: usize = 200;
const LLM_PROBE_MAX_TOKENS: u32 = 16;
const KEYRING_TIMEOUT: Duration = Duration::from_secs(3);

/// The answer of either test. A failure is data, not an IPC error: the window needs the field
/// and the kind of the failure to name the next step in its own language
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeOutcome<T> {
    pub report: Option<T>,
    pub error: Option<ProbeFailure>,
}

impl<T> ProbeOutcome<T> {
    fn report(report: T) -> Self {
        Self {
            report: Some(report),
            error: None,
        }
    }
}

/// Why a draft check failed. The window turns `key` into a translated message
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeFailure {
    Validation {
        field: String,
        key: String,
        value: String,
    },
    Consent,
    Llm(LlmError),
}

/// What the model test found, as the settings page shows it
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProbeReport {
    pub provider: String,
    pub model: String,
    pub is_local: bool,
    pub latency_ms: u64,
    pub attempts: u32,
    pub answer: String,
}

/// One line of the recognition test
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttProbeCheck {
    pub id: String,
    pub ok: bool,
    /// A path or an error. A translated `detail_key` wins when the window has one
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_key: Option<String>,
}

/// What the recognition test found
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttProbeReport {
    pub engine: String,
    pub model: String,
    pub ok: bool,
    pub checks: Vec<SttProbeCheck>,
}

fn request_for(cfg: &LlmConfig) -> ChatRequest {
    ChatRequest {
        model: cfg.model.clone(),
        system: LLM_PROBE_SYSTEM.into(),
        user: LLM_PROBE_PROMPT.into(),
        temperature: 0.0,
        max_tokens: LLM_PROBE_MAX_TOKENS,
    }
}

/// One request under the retry policy of the pipeline: an authorization error is not retried,
/// an empty answer counts as a failure, and `max_retries` more requests follow the first
fn complete_with_retries(
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
            Err(err @ (LlmError::Auth | LlmError::Consent)) => return Err(err),
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// The key as the daemon would read it, with a bound on the OS key store
fn probe_api_key(cfg: &LlmConfig) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    let probe = cfg.clone();
    std::thread::spawn(move || {
        let _ = tx.send(secrets::api_key(&probe));
    });
    rx.recv_timeout(KEYRING_TIMEOUT)
        .unwrap_or_else(|_| secrets::api_key_with(cfg, |name| std::env::var(name).ok()))
}

/// Run the draft model configuration against a harmless prompt
pub fn llm_probe(config: &Config) -> ProbeOutcome<LlmProbeReport> {
    let cfg = &config.llm;
    if let Some(token) = invalid_provider_slug(&cfg.provider_order) {
        return ProbeOutcome {
            report: None,
            error: Some(ProbeFailure::Validation {
                field: "llm.provider_order".into(),
                key: "validation.providerSlug".into(),
                value: token,
            }),
        };
    }
    if cfg.model.trim().is_empty() {
        return ProbeOutcome {
            report: None,
            error: Some(ProbeFailure::Validation {
                field: "llm.model".into(),
                key: "validation.noLlmModel".into(),
                value: String::new(),
            }),
        };
    }
    let client = match OpenAiCompatClient::from_config(cfg, probe_api_key(cfg)) {
        Ok(client) => client.with_cloud_consent(config.privacy.cloud_consent),
        Err(err) => {
            return ProbeOutcome {
                report: None,
                error: Some(ProbeFailure::Llm(err)),
            }
        }
    };
    let (answer, attempts, latency_ms) =
        match complete_with_retries(&client, &request_for(cfg), cfg.max_retries) {
            Ok(result) => result,
            Err(err) => {
                return ProbeOutcome {
                    report: None,
                    error: Some(if err == LlmError::Consent {
                        ProbeFailure::Consent
                    } else {
                        ProbeFailure::Llm(err)
                    }),
                }
            }
        };
    ProbeOutcome::report(LlmProbeReport {
        provider: client.id().to_string(),
        model: cfg.model.trim().to_string(),
        is_local: client.is_local(),
        latency_ms,
        attempts,
        answer: answer.trim().chars().take(LLM_PROBE_ANSWER).collect(),
    })
}

/// The directory of the running binary: an AppImage and a package keep their helpers beside it,
/// and no such directory is in `PATH`
fn executable_directory() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// The program that the recognition engine will execute, in the order the engine tries it: the
/// value of `stt.transcribe_cli`, a copy beside the running binary, then `PATH`
pub fn transcribe_cli_program(configured: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH");
    transcribe_cli_at(
        configured,
        executable_directory().as_deref(),
        path.as_deref(),
    )
}

/// The same search with its environment in the arguments: a test places the program in a
/// directory of its own instead of depending on where the test binary stands and on `PATH`
fn transcribe_cli_at(
    configured: &str,
    exe_dir: Option<&Path>,
    path: Option<&OsStr>,
) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let named = expand_tilde(configured);
        return named.is_file().then_some(named);
    }
    let neighbour = exe_dir
        .map(|directory| directory.join(TRANSCRIBE_CLI))
        .filter(|candidate| candidate.is_file());
    neighbour.or_else(|| program_in_dirs(TRANSCRIBE_CLI, std::env::split_paths(path?)))
}

fn program_in_dirs(program: &str, directories: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    directories
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

fn expand_tilde(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if let (Some(rest), Some(home)) = (trimmed.strip_prefix('~'), std::env::var_os("HOME")) {
        let rest = rest.trim_start_matches('/');
        return if rest.is_empty() {
            PathBuf::from(home)
        } else {
            PathBuf::from(home).join(rest)
        };
    }
    PathBuf::from(trimmed)
}

/// The program that the recognition engine will execute
fn transcribe_cli_check(configured: &str) -> SttProbeCheck {
    match transcribe_cli_program(configured) {
        Some(path) => SttProbeCheck {
            id: "cli".into(),
            ok: true,
            detail: path.display().to_string(),
            detail_key: None,
        },
        None => SttProbeCheck {
            id: "cli".into(),
            ok: false,
            detail: String::new(),
            detail_key: Some("settings.sttTest.cliMissing".into()),
        },
    }
}

/// Check the draft recognition settings without recording anything
///
/// The engine is built and dropped: building checks the engine name and the weights of a local
/// engine, loads no model and opens no device, so the microphone stays untouched
pub fn stt_probe(config: &Config) -> ProbeOutcome<SttProbeReport> {
    let model = config.stt.model.trim().to_string();
    let engine = engine::resolve_engine(config, &engine::EngineChoice::default());
    let mut checks: Vec<SttProbeCheck> = Vec::new();
    match engine::build_stt_with(config, &engine::EngineChoice::default()) {
        Ok(built) => checks.push(SttProbeCheck {
            id: "engine".into(),
            ok: true,
            detail: built.id().to_string(),
            detail_key: None,
        }),
        Err(engine::EngineError::NotAvailable(_)) => checks.push(SttProbeCheck {
            id: "remote".into(),
            ok: false,
            detail: String::new(),
            detail_key: Some("settings.sttTest.remoteUnavailable".into()),
        }),
        Err(error) => checks.push(SttProbeCheck {
            id: "engine".into(),
            ok: false,
            detail: error.to_string(),
            detail_key: None,
        }),
    }
    if engine != "remote-openai" {
        match models::installed_path(config, &model) {
            Ok(path) => checks.push(SttProbeCheck {
                id: "model".into(),
                ok: true,
                detail: path.display().to_string(),
                detail_key: None,
            }),
            Err(error) => checks.push(SttProbeCheck {
                id: "model".into(),
                ok: false,
                detail: error.to_string(),
                detail_key: None,
            }),
        }
        if engine == "transcribe-cpp" {
            checks.push(transcribe_cli_check(&config.stt.transcribe_cli));
        }
    }
    ProbeOutcome::report(SttProbeReport {
        engine,
        model,
        ok: checks.iter().all(|check| check.ok),
        checks,
    })
}

/// The shape of the IPC answer, for a caller that only needs the raw JSON
pub fn llm_probe_json(config: &Config) -> Result<Value, serde_json::Error> {
    serde_json::to_value(llm_probe(config))
}

pub fn stt_probe_json(config: &Config) -> Result<Value, serde_json::Error> {
    serde_json::to_value(stt_probe(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::domain::fakes::FakeLlm;
    use crate::domain::llm::ChatResponse;

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
    fn the_request_carries_no_dictated_text() {
        let request = request_for(&local_draft("http://127.0.0.1:9/v1").llm);
        assert_eq!(request.user, LLM_PROBE_PROMPT);
        assert_eq!(request.max_tokens, LLM_PROBE_MAX_TOKENS);
        assert_eq!(request.temperature, 0.0);
    }

    #[test]
    fn a_rejected_key_is_not_retried() {
        let client = FakeLlm::failing(LlmError::Auth);
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let error =
            complete_with_retries(&client, &request_for(&cfg), cfg.max_retries).unwrap_err();
        assert_eq!(error, LlmError::Auth);
        assert_eq!(client.calls(), 1, "a retry does not fix the key");
    }

    #[test]
    fn the_retry_budget_is_spent_and_never_exceeded() {
        let client = FakeLlm::failing(LlmError::Unavailable("model is loading".into()));
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let error =
            complete_with_retries(&client, &request_for(&cfg), cfg.max_retries).unwrap_err();
        assert!(matches!(error, LlmError::Unavailable(_)), "{error:?}");
        assert_eq!(client.calls(), 3);
    }

    #[test]
    fn an_empty_answer_is_a_failure_and_a_retry() {
        let client = FakeLlm::new(|_| {
            Ok(ChatResponse {
                text: "   ".into(),
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd: None,
                note: None,
            })
        });
        let cfg = local_draft("http://127.0.0.1:9/v1").llm;
        let error = complete_with_retries(&client, &request_for(&cfg), 1).unwrap_err();
        assert!(matches!(error, LlmError::BadResponse(_)), "{error:?}");
        assert_eq!(client.calls(), 2);
    }

    #[test]
    fn a_cloud_draft_needs_the_cloud_consent() {
        let mut config = local_draft("https://openrouter.ai/api/v1");
        config.llm.provider = "openrouter".into();
        config.privacy.cloud_consent = false;
        assert_eq!(llm_probe(&config).error, Some(ProbeFailure::Consent));
    }

    #[test]
    fn a_remote_draft_needs_consent_regardless_of_provider_name() {
        for provider in ["custom", "ollama", "lmstudio"] {
            let mut config = local_draft("http://0.0.0.0:9/v1");
            config.llm.provider = provider.into();
            assert_eq!(llm_probe(&config).error, Some(ProbeFailure::Consent));
        }
    }

    #[test]
    fn a_draft_without_a_model_names_the_field() {
        let mut config = local_draft("http://127.0.0.1:9/v1");
        config.llm.model = "  ".into();
        assert_eq!(
            llm_probe(&config).error,
            Some(ProbeFailure::Validation {
                field: "llm.model".into(),
                key: "validation.noLlmModel".into(),
                value: String::new(),
            })
        );
    }

    #[test]
    fn a_hoster_order_that_is_not_a_list_of_slugs_is_refused() {
        let mut config = local_draft("https://openrouter.ai/api/v1");
        config.llm.provider = "openrouter".into();
        config.privacy.cloud_consent = true;
        config.llm.provider_order = "groq; together".into();
        assert!(matches!(
            llm_probe(&config).error,
            Some(ProbeFailure::Validation { ref field, .. }) if field == "llm.provider_order"
        ));
    }

    #[test]
    fn a_dead_local_model_becomes_a_model_failure() {
        // A closed port on this machine: no outside network is touched
        let config = local_draft("http://127.0.0.1:9/v1");
        let error = llm_probe(&config).error.expect("the probe must fail");
        assert!(matches!(error, ProbeFailure::Llm(_)), "{error:?}");
    }

    fn models_with_default_weights(directory: &Path) -> Config {
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
        let report = stt_probe(&config).report.unwrap();
        assert_eq!(report.engine, "transcribe-cpp");
        assert!(!report.ok);
        let ids: Vec<&str> = report.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["engine", "model", "cli"]);
        assert!(report.checks.iter().any(|c| !c.ok));
    }

    #[test]
    fn the_recognition_test_passes_when_the_weights_and_the_cli_are_there() {
        let directory = tempfile::tempdir().unwrap();
        let cli = directory.path().join("transcribe-cli");
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        let mut config = models_with_default_weights(directory.path());
        config.stt.transcribe_cli = cli.display().to_string();
        let report = stt_probe(&config).report.unwrap();
        assert!(report.ok, "{report:?}");
        let model = report.checks.iter().find(|c| c.id == "model").unwrap();
        assert!(model.ok);
    }

    #[test]
    fn remote_recognition_is_reported_as_not_implemented() {
        let mut config = Config::default();
        config.stt.engine = "remote-openai".into();
        let report = stt_probe(&config).report.unwrap();
        assert!(!report.ok);
        assert_eq!(report.engine, "remote-openai");
        assert_eq!(report.checks.len(), 1, "{report:?}");
    }

    /// A stand-in for the transcribe.cpp program: the search looks at the file alone
    fn program(directory: &Path) -> PathBuf {
        let path = directory.join(TRANSCRIBE_CLI);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        path
    }

    #[test]
    fn the_named_program_wins_over_a_neighbour_and_the_path() {
        let named = tempfile::tempdir().unwrap();
        let beside = tempfile::tempdir().unwrap();
        let in_path = tempfile::tempdir().unwrap();
        let named_program = program(named.path());
        program(beside.path());
        program(in_path.path());
        let found = transcribe_cli_at(
            &named_program.display().to_string(),
            Some(beside.path()),
            Some(in_path.path().as_os_str()),
        );
        assert_eq!(found, Some(named_program));
    }

    #[test]
    fn a_program_beside_the_binary_is_found_with_an_empty_setting_and_path() {
        let beside = tempfile::tempdir().unwrap();
        let empty_path = tempfile::tempdir().unwrap();
        let expected = program(beside.path());
        for configured in ["", "   "] {
            let found = transcribe_cli_at(
                configured,
                Some(beside.path()),
                Some(empty_path.path().as_os_str()),
            );
            assert_eq!(found, Some(expected.clone()), "{configured:?}");
        }
    }

    #[test]
    fn the_path_is_the_last_place_searched() {
        let beside = tempfile::tempdir().unwrap();
        let in_path = tempfile::tempdir().unwrap();
        let expected = program(in_path.path());
        let found = transcribe_cli_at("", Some(beside.path()), Some(in_path.path().as_os_str()));
        assert_eq!(found, Some(expected));
    }

    #[test]
    fn a_program_nowhere_is_reported_as_missing() {
        let beside = tempfile::tempdir().unwrap();
        let empty_path = tempfile::tempdir().unwrap();
        assert_eq!(
            transcribe_cli_at("", Some(beside.path()), Some(empty_path.path().as_os_str())),
            None
        );
        assert_eq!(transcribe_cli_at("", None, None), None);
    }
}
