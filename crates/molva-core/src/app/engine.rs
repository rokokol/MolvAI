// SPDX-License-Identifier: MIT
//! Recognizer factory: builds a concrete `SttEngine` from the settings
//!
//! This is the only place where the engine name from the config becomes an object. The CLI,
//! the daemon and the bench all come here, so the "model not downloaded" hint reads the same
//! at every entry point

use thiserror::Error;

use crate::app::models::{self, ModelError};
use crate::app::probe;
use crate::app::secrets;
use crate::config::{Config, RemoteSttConfig};
use crate::domain::fakes::FakeStt;
use crate::domain::stt::SttEngine;
use crate::infra::stt::RemoteOpenAiEngine;
use crate::infra::stt::TranscribeCppEngine;
use crate::infra::stt::WhisperEngine;

/// Text that the fake engine returns when no other text is set
pub const DEFAULT_FAKE_TEXT: &str = "тестовая расшифровка";

/// Name of the fake engine: `MOLVA_STT=fake`, `--engine fake`, `stt.engine = "fake"`
pub const FAKE_ENGINE: &str = "fake";

/// Name of the cloud recognizer: an OpenAI-compatible `/audio/transcriptions` endpoint
pub const REMOTE_ENGINE: &str = "remote-openai";

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown recognition engine: {0}")]
    Unknown(String),
    #[error("engine not available in this build: {0}")]
    NotAvailable(String),
    /// The program that the engine shells out to is not on this machine: the weights alone load
    /// nothing, so the failure belongs to the build and not to the first phrase
    #[error(
        "cannot find transcribe-cli{wanted}, which runs the model {model}: install transcribe.cpp, \
         set stt.transcribe_cli to the binary, or choose a whisper model in stt.model"
    )]
    CliMissing { model: String, wanted: String },
    /// The cloud recognizer is missing something it cannot run without: the address, the model or
    /// the key. Each names the setting and the next step, so the user fixes it before a phrase
    #[error("cannot use remote recognition: {0}")]
    RemoteConfig(String),
    #[error(transparent)]
    Model(#[from] ModelError),
}

/// Overrides from the command line and the environment; empty fields mean "as in the config"
///
/// The CLI reads the environment variables and puts them here. The core does not look at the
/// environment, or the tests would need one process per case
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineChoice {
    pub engine: Option<String>,
    pub model: Option<String>,
    /// Reply of the fake engine; only for tests and the bench self-check
    pub fake_text: Option<String>,
}

impl EngineChoice {
    pub fn fake() -> Self {
        Self {
            engine: Some(FAKE_ENGINE.to_string()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: Option<&str>) -> Self {
        self.model = model.map(str::to_string);
        self
    }
}

/// Build the engine from the config, with a model override
pub fn build_stt(
    cfg: &Config,
    override_model: Option<&str>,
) -> Result<Box<dyn SttEngine>, EngineError> {
    build_stt_with(cfg, &EngineChoice::default().with_model(override_model))
}

/// The engine that the settings resolve to, before any engine is built
///
/// `auto` follows the catalog: the model says which runtime can load it, and the user does not
/// have to know that GigaAM is not a whisper file. The factory and a diagnostic both ask here,
/// so the settings page never names an engine the factory would not build
pub fn resolve_engine(cfg: &Config, choice: &EngineChoice) -> String {
    let requested = choice.engine.as_deref().unwrap_or(&cfg.stt.engine).trim();
    let model = choice.model.as_deref().unwrap_or(&cfg.stt.model).trim();
    if requested == "auto" {
        models::find(model).map_or_else(|_| "whisper-cpp".to_string(), |info| info.engine.clone())
    } else {
        requested.to_string()
    }
}

/// Where the settings name the program, when they name a place
fn configured_cli(configured: &str) -> String {
    match configured.trim() {
        "" => String::new(),
        path => format!(" at {path}"),
    }
}

/// Build the engine with the full set of overrides
pub fn build_stt_with(
    cfg: &Config,
    choice: &EngineChoice,
) -> Result<Box<dyn SttEngine>, EngineError> {
    let engine = resolve_engine(cfg, choice);
    let model = choice.model.as_deref().unwrap_or(&cfg.stt.model).trim();

    match engine.as_str() {
        FAKE_ENGINE => Ok(Box::new(FakeStt::returning(
            choice.fake_text.as_deref().unwrap_or(DEFAULT_FAKE_TEXT),
        ))),
        "whisper-cpp" => {
            // The model file is checked before the load attempt: the user needs the `pull`
            // command, not a library error about an unreadable file. The whisper context
            // itself loads lazily, at the first phrase
            let path = models::installed_path(cfg, model)?;
            Ok(Box::new(WhisperEngine::new(
                path,
                model.to_string(),
                cfg.stt.threads as usize,
            )))
        }
        "transcribe-cpp" => {
            // The weights are half of what this engine needs; the program it runs is the other
            // half. A missing program failed every phrase with one error after a start that
            // looked healthy. The found path goes into the engine, because the search also
            // reaches beside the running binary, and the bare name would not
            let program =
                probe::transcribe_cli_program(&cfg.stt.transcribe_cli).ok_or_else(|| {
                    EngineError::CliMissing {
                        model: model.to_string(),
                        wanted: configured_cli(&cfg.stt.transcribe_cli),
                    }
                })?;
            let path = models::installed_path(cfg, model)?;
            Ok(Box::new(TranscribeCppEngine::new(
                path,
                model.to_string(),
                program.display().to_string(),
                cfg.stt.transcribe_backend.clone(),
                cfg.stt.threads as usize,
            )))
        }
        REMOTE_ENGINE => {
            // The engine is a client of a provider: nothing is downloaded, nothing is loaded, and
            // every reason it cannot run is a setting the user owns
            let remote = &cfg.stt.remote;
            if remote.base_url.trim().is_empty() {
                return Err(EngineError::RemoteConfig(
                    "stt.remote.base_url is empty: set the address of the recognition API".into(),
                ));
            }
            if remote.model.trim().is_empty() {
                return Err(EngineError::RemoteConfig(
                    "stt.remote.model is empty: set the model of the recognition API, for example \
                     whisper-large-v3-turbo"
                        .into(),
                ));
            }
            let key = secrets::remote_stt_api_key(remote)
                .ok_or_else(|| EngineError::RemoteConfig(missing_remote_key(remote)))?;
            let engine = RemoteOpenAiEngine::from_remote(remote, &key, cfg.privacy.cloud_consent)
                .map_err(|err| EngineError::RemoteConfig(err.to_string()))?;
            Ok(Box::new(engine))
        }
        other => Err(EngineError::Unknown(other.to_string())),
    }
}

/// Why no key was found, and the two places that can hold one
fn missing_remote_key(remote: &RemoteSttConfig) -> String {
    if remote.api_key_source.trim().eq_ignore_ascii_case("none") {
        return "stt.remote.api_key_source is `none`, so no key is read: choose `keyring` or `env`, \
                or turn off stt.engine = \"remote-openai\""
            .to_string();
    }
    let variable = remote.api_key_env.trim();
    if variable.is_empty() {
        return format!(
            "no key: add the entry {} to the key store, or set stt.remote.api_key_env to the name \
             of a variable that holds it",
            secrets::STT_KEY_ENTRY
        );
    }
    format!(
        "no key: set the environment variable {variable} (stt.remote.api_key_env), or add the \
         entry {} to the key store of stt.remote.api_key_source",
        secrets::STT_KEY_ENTRY
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::audio::PcmAudio;
    use crate::domain::stt::SttOptions;

    fn cfg_with_models_dir(directory: &std::path::Path) -> Config {
        let mut cfg = Config::default();
        cfg.stt.model_path = directory.display().to_string();
        cfg
    }

    #[test]
    fn fake_engine_is_built_from_config() {
        let mut cfg = Config::default();
        cfg.stt.engine = FAKE_ENGINE.into();
        let mut engine = build_stt(&cfg, None).unwrap();
        assert_eq!(engine.id(), "fake");
        let audio = PcmAudio::new(vec![0.1; 16_000], 16_000);
        assert_eq!(
            engine
                .transcribe(&audio, &SttOptions::default())
                .unwrap()
                .text,
            DEFAULT_FAKE_TEXT
        );
    }

    #[test]
    fn engine_override_beats_config() {
        let cfg = Config::default(); // engine = whisper-cpp
        let choice = EngineChoice {
            engine: Some(FAKE_ENGINE.into()),
            fake_text: Some("привет".into()),
            ..EngineChoice::default()
        };
        let mut engine = build_stt_with(&cfg, &choice).unwrap();
        let audio = PcmAudio::new(vec![0.1; 100], 16_000);
        assert_eq!(
            engine
                .transcribe(&audio, &SttOptions::default())
                .unwrap()
                .text,
            "привет"
        );
    }

    #[test]
    fn missing_weights_tell_the_user_how_to_get_them() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = cfg_with_models_dir(directory.path());
        let err = build_stt(&cfg, Some("small")).unwrap_err();
        assert!(err.to_string().contains("molva models pull small"), "{err}");
    }

    #[test]
    fn unknown_model_name_is_reported_before_anything_else() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = cfg_with_models_dir(directory.path());
        let err = build_stt(&cfg, Some("gigantic")).unwrap_err();
        assert!(err.to_string().contains("gigantic"), "{err}");
    }

    #[test]
    fn present_weights_build_a_lazy_whisper_engine() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("ggml-tiny.bin"), "not real weights").unwrap();
        let cfg = cfg_with_models_dir(directory.path());
        // The weights are not real, but the context loads lazily. Building the engine must
        // succeed, and the error about the broken file comes only at the first phrase
        let engine = match build_stt(&cfg, Some("tiny")) {
            Ok(engine) => engine,
            Err(err) => panic!("the whisper engine must build: {err}"),
        };
        assert_eq!(engine.id(), "whisper-cpp");
        assert_eq!(engine.model_name(), "tiny");
    }

    #[test]
    fn unknown_engine_name_is_rejected() {
        let mut cfg = Config::default();
        cfg.stt.engine = "vosk".into();
        let err = build_stt(&cfg, None).unwrap_err();
        assert!(matches!(err, EngineError::Unknown(_)), "{err}");
    }

    #[test]
    fn auto_engine_follows_the_catalog() {
        let directory = tempfile::tempdir().unwrap();
        let mut cfg = gigaam_config(directory.path());
        cfg.stt.engine = "auto".into();
        let cli = directory.path().join("transcribe-cli");
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        cfg.stt.transcribe_cli = cli.display().to_string();
        let engine = build_stt(&cfg, None).expect("the GigaAM weights are in place");
        assert_eq!(engine.id(), "transcribe-cpp");
    }

    /// The default model with its weights in place: only the program it runs varies
    fn gigaam_config(directory: &std::path::Path) -> Config {
        std::fs::write(directory.join("gigaam-v3-e2e-rnnt-Q4_K_M.gguf"), "weights").unwrap();
        let mut cfg = cfg_with_models_dir(directory);
        cfg.stt.model = "gigaam-v3-e2e-rnnt-q4_k_m".into();
        cfg
    }

    #[test]
    fn the_default_model_without_its_program_fails_before_the_first_phrase() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = gigaam_config(directory.path());
        // The premise of this case is a machine without transcribe.cpp. The search reads PATH
        // and the directory of this test binary, and the test controls neither
        assert!(
            probe::transcribe_cli_program("").is_none(),
            "transcribe-cli is installed on this machine: the case needs it absent"
        );
        let err = build_stt(&cfg, None).unwrap_err().to_string();
        assert!(err.contains("transcribe-cli"), "{err}");
        assert!(err.contains("stt.transcribe_cli"), "{err}");
        assert!(err.contains("stt.model"), "{err}");
    }

    #[test]
    fn a_named_program_that_is_absent_is_named_in_the_error() {
        let directory = tempfile::tempdir().unwrap();
        let mut cfg = gigaam_config(directory.path());
        let missing = directory.path().join("no-such-cli");
        cfg.stt.transcribe_cli = missing.display().to_string();
        let err = build_stt(&cfg, None).unwrap_err().to_string();
        assert!(err.contains("transcribe-cli"), "{err}");
        assert!(err.contains(&missing.display().to_string()), "{err}");
    }

    #[test]
    fn a_present_program_builds_the_gigaam_engine() {
        let directory = tempfile::tempdir().unwrap();
        let mut cfg = gigaam_config(directory.path());
        let cli = directory.path().join("transcribe-cli");
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        cfg.stt.transcribe_cli = cli.display().to_string();
        let engine = build_stt(&cfg, None).expect("the weights and the program are in place");
        assert_eq!(engine.id(), "transcribe-cpp");
    }

    #[test]
    fn a_whisper_model_is_built_without_any_transcribe_cli() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("ggml-tiny.bin"), "not real weights").unwrap();
        let cfg = cfg_with_models_dir(directory.path());
        assert!(cfg.stt.transcribe_cli.is_empty());
        let engine = build_stt(&cfg, Some("tiny")).expect("whisper needs no external program");
        assert_eq!(engine.id(), "whisper-cpp");
    }

    #[test]
    fn the_resolved_engine_is_named_without_building_it() {
        // A diagnostic that reports the engine of a draft must not need the weights, and must
        // not guess a name the factory would not build
        let directory = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_models_dir(directory.path());
        cfg.stt.model = "gigaam-v3-e2e-rnnt-q4_k_m".into();
        cfg.stt.engine = "auto".into();
        assert_eq!(
            resolve_engine(&cfg, &EngineChoice::default()),
            "transcribe-cpp"
        );

        cfg.stt.model = "small".into();
        assert_eq!(
            resolve_engine(&cfg, &EngineChoice::default()),
            "whisper-cpp"
        );

        cfg.stt.model = "no-such-model".into();
        assert_eq!(
            resolve_engine(&cfg, &EngineChoice::default()),
            "whisper-cpp"
        );

        cfg.stt.engine = "transcribe-cpp".into();
        assert_eq!(
            resolve_engine(&cfg, &EngineChoice::default()),
            "transcribe-cpp"
        );

        let choice = EngineChoice::fake();
        assert_eq!(resolve_engine(&cfg, &choice), FAKE_ENGINE);
    }

    /// Remote recognition from the settings: the address, the model and the key are checked
    /// before anything is sent, so a broken draft fails at the start and not at the first phrase
    #[test]
    fn a_remote_engine_names_the_setting_that_stops_it() {
        let mut cfg = Config::default();
        cfg.stt.engine = "remote-openai".into();
        cfg.stt.remote.base_url = "   ".into();
        let err = build_stt(&cfg, None).unwrap_err().to_string();
        assert!(err.contains("stt.remote.base_url"), "{err}");

        cfg.stt.remote.base_url = "http://127.0.0.1:9/v1".into();
        cfg.stt.remote.model = "  ".into();
        let err = build_stt(&cfg, None).unwrap_err().to_string();
        assert!(err.contains("stt.remote.model"), "{err}");

        // No key: the environment variable of the settings is the next step, and it is named
        cfg.stt.remote.model = "whisper-large-v3-turbo".into();
        cfg.stt.remote.api_key_source = "env".into();
        cfg.stt.remote.api_key_env = "MOLVA_TEST_ABSENT_STT_KEY".into();
        let err = build_stt(&cfg, None).unwrap_err().to_string();
        assert!(err.contains("no key"), "{err}");
        assert!(err.contains("MOLVA_TEST_ABSENT_STT_KEY"), "{err}");

        // A key source that reads nothing says so instead of blaming the environment
        cfg.stt.remote.api_key_source = "none".into();
        let err = build_stt(&cfg, None).unwrap_err().to_string();
        assert!(err.contains("stt.remote.api_key_source"), "{err}");
    }

    /// The engine is built from the settings and the key, and the consent of the settings reaches
    /// it. The key is in the environment, so the case is a child process of its own: the tests of
    /// one run share one environment
    #[test]
    fn the_remote_engine_is_built_from_the_settings_the_key_and_the_consent() {
        const CHILD: &str = "MOLVA_STT_REMOTE_CHILD";
        const KEY: &str = "sk-child-0123456789abcdef";
        if std::env::var(CHILD).is_ok() {
            let mut cfg = Config::default();
            cfg.stt.engine = "remote-openai".into();
            // Not this machine: without the consent of the settings nothing may be sent there
            cfg.stt.remote.base_url = "http://0.0.0.0:9/v1".into();
            cfg.stt.remote.api_key_source = "env".into();
            cfg.stt.remote.api_key_env = "MOLVA_TEST_STT_KEY".into();
            let audio = PcmAudio::new(vec![0.1; 160], 16_000);

            let mut engine = build_stt(&cfg, None).expect("the key is in the environment");
            assert_eq!(engine.id(), "remote-openai");
            assert_eq!(engine.model_name(), "remote-openai");
            let refused = engine
                .transcribe(&audio, &SttOptions::default())
                .unwrap_err()
                .to_string();
            assert!(refused.contains("cloud_consent"), "{refused}");

            cfg.privacy.cloud_consent = true;
            let mut engine = build_stt(&cfg, None).expect("the key is in the environment");
            let tried = engine
                .transcribe(&audio, &SttOptions::default())
                .unwrap_err()
                .to_string();
            assert!(
                !tried.contains("cloud_consent"),
                "the consent of the settings must reach the engine: {tried}"
            );
            let printed = format!("{engine:?}");
            assert!(!printed.contains(KEY), "{printed}");
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::engine::tests::the_remote_engine_is_built_from_the_settings_the_key_and_the_consent",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("MOLVA_TEST_STT_KEY", KEY)
            // No proxy: the case needs a refusal of the connection, not a wait on another machine
            .env_remove("HTTP_PROXY")
            .env_remove("http_proxy")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
