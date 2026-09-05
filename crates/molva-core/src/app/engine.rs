// SPDX-License-Identifier: MIT
//! Фабрика распознавателей: по настройкам собирает конкретный `SttEngine`.
//!
//! Единственное место, где имя движка из конфига превращается в объект. CLI, демон и бенч
//! ходят сюда, поэтому подсказка «модель не скачана» звучит одинаково во всех входах.

use thiserror::Error;

use crate::app::models::{self, ModelError};
use crate::config::Config;
use crate::domain::fakes::FakeStt;
use crate::domain::stt::SttEngine;
use crate::infra::stt::WhisperEngine;

/// Текст, который отдаёт фейковый движок, если не задан свой.
pub const DEFAULT_FAKE_TEXT: &str = "тестовая расшифровка";

/// Имя фейкового движка: `MOLVA_STT=fake`, `--engine fake`, `stt.engine = "fake"`.
pub const FAKE_ENGINE: &str = "fake";

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("неизвестный движок распознавания: {0}")]
    Unknown(String),
    #[error("движок недоступен в этой сборке: {0}")]
    NotAvailable(String),
    #[error(transparent)]
    Model(#[from] ModelError),
}

/// Переопределения из командной строки и окружения; пустые поля означают «как в конфиге».
///
/// Переменные окружения читает CLI и кладёт сюда — ядро в окружение не заглядывает,
/// иначе тесты пришлось бы гонять по одному процессу на случай.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineChoice {
    pub engine: Option<String>,
    pub model: Option<String>,
    /// Ответ фейкового движка; только для тестов и самопроверки бенча.
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

/// Собрать движок по конфигу с переопределением модели.
pub fn build_stt(
    cfg: &Config,
    override_model: Option<&str>,
) -> Result<Box<dyn SttEngine>, EngineError> {
    build_stt_with(cfg, &EngineChoice::default().with_model(override_model))
}

/// Собрать движок с полным набором переопределений.
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
            // Файл модели проверяем до попытки загрузки: пользователю нужна команда `pull`,
            // а не ошибка библиотеки о нечитаемом файле. Сам контекст whisper грузится лениво,
            // при первой реплике.
            let path = models::installed_path(cfg, model)?;
            Ok(Box::new(WhisperEngine::new(
                path,
                model.to_string(),
                cfg.stt.threads as usize,
            )))
        }
        "remote-openai" => Err(EngineError::NotAvailable(
            "удалённое распознавание подключается отдельно; используйте stt.engine = \"whisper-cpp\""
                .into(),
        )),
        other => Err(EngineError::Unknown(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::audio::PcmAudio;
    use crate::domain::stt::SttOptions;

    fn cfg_with_models_dir(dir: &std::path::Path) -> Config {
        let mut cfg = Config::default();
        cfg.stt.model_path = dir.display().to_string();
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
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with_models_dir(dir.path());
        let err = build_stt(&cfg, Some("small")).unwrap_err();
        assert!(err.to_string().contains("molva models pull small"), "{err}");
    }

    #[test]
    fn unknown_model_name_is_reported_before_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with_models_dir(dir.path());
        let err = build_stt(&cfg, Some("gigantic")).unwrap_err();
        assert!(err.to_string().contains("gigantic"), "{err}");
    }

    #[test]
    fn present_weights_build_a_lazy_whisper_engine() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-tiny.bin"), "не настоящие веса").unwrap();
        let cfg = cfg_with_models_dir(dir.path());
        // Веса ненастоящие, но контекст грузится лениво: сборка движка обязана пройти,
        // а ошибка про битый файл придёт только при первой реплике.
        let engine = match build_stt(&cfg, Some("tiny")) {
            Ok(engine) => engine,
            Err(err) => panic!("движок whisper обязан собираться: {err}"),
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
