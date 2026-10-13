// SPDX-License-Identifier: MIT
//! Recognizer factory: builds a concrete `SttEngine` from the settings
//!
//! This is the only place where the engine name from the config becomes an object. The CLI,
//! the daemon and the bench all come here, so the "model not downloaded" hint reads the same
//! at every entry point

use thiserror::Error;

use crate::app::models::{self, ModelError};
use crate::config::Config;
use crate::domain::fakes::FakeStt;
use crate::domain::stt::SttEngine;
use crate::infra::stt::WhisperEngine;

/// Text that the fake engine returns when no other text is set
pub const DEFAULT_FAKE_TEXT: &str = "тестовая расшифровка";

/// Name of the fake engine: `MOLVA_STT=fake`, `--engine fake`, `stt.engine = "fake"`
pub const FAKE_ENGINE: &str = "fake";

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown recognition engine: {0}")]
    Unknown(String),
    #[error("engine not available in this build: {0}")]
    NotAvailable(String),
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

/// Build the engine with the full set of overrides
pub fn build_stt_with(
    cfg: &Config,
    choice: &EngineChoice,
) -> Result<Box<dyn SttEngine>, EngineError> {
    let engine = choice.engine.as_deref().unwrap_or(&cfg.stt.engine).trim();
    let model = choice.model.as_deref().unwrap_or(&cfg.stt.model).trim();

    match engine {
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
        "remote-openai" => Err(EngineError::NotAvailable(
            "remote recognition is wired up separately; use stt.engine = \"whisper-cpp\"".into(),
        )),
        other => Err(EngineError::Unknown(other.to_string())),
    }
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
        // The weights are not real, but the context loads lazily: building the engine must
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
}
