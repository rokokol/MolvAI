// SPDX-License-Identifier: MIT
//! Распознавание речи: параметры, результат и контракт движка.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::audio::PcmAudio;

/// Подсказка языка для распознавателя.
///
/// `Fixed` отключает автоопределение; `Auto` отдаёт выбор модели, а приложение затем сверяет
/// результат со списком разрешённых языков и при необходимости делает один повтор.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageHint {
    Auto,
    Fixed(String),
}

impl LanguageHint {
    /// Разбор значения из конфига: `"auto"` или код ISO-639-1.
    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            LanguageHint::Auto
        } else {
            LanguageHint::Fixed(value.to_lowercase())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttOptions {
    pub language: LanguageHint,
    /// Языки, которые пользователь реально использует; при `Auto` детект вне списка → повтор.
    pub allowed_languages: Vec<String>,
    /// Подсказка модели: термины, имена, топонимы.
    pub initial_prompt: Option<String>,
    /// 0 = все логические ядра.
    pub threads: usize,
    /// Нужны ли сегменты с таймкодами.
    pub timestamps: bool,
}

impl Default for SttOptions {
    fn default() -> Self {
        Self {
            language: LanguageHint::Auto,
            allowed_languages: vec!["ru".into(), "en".into()],
            initial_prompt: None,
            threads: 0,
            timestamps: false,
        }
    }
}

/// Фрагмент распознанного текста с таймкодами относительно начала аудио.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub segments: Vec<Segment>,
    /// Определённый моделью язык (ISO-639-1), если движок его сообщает.
    pub detected_language: Option<String>,
    /// Вероятность того, что речи не было: используется против галлюцинаций на тишине.
    pub no_speech_prob: Option<f32>,
}

impl Transcript {
    pub fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SttError {
    #[error("файл модели не найден: {path}. Скачайте: molva models pull {model}")]
    ModelMissing { path: String, model: String },
    #[error("не удалось загрузить модель: {0}")]
    ModelLoad(String),
    #[error("ошибка распознавания: {0}")]
    Inference(String),
    #[error("пустое аудио")]
    EmptyAudio,
    #[error("движок не поддерживает: {0}")]
    Unsupported(String),
}

/// Движок распознавания: whisper.cpp в проде, `FakeStt` в тестах.
pub trait SttEngine: Send {
    /// Идентификатор движка для журнала, например `whisper-cpp`.
    fn id(&self) -> &str;
    /// Имя модели для журнала, например `small`.
    fn model_name(&self) -> &str;
    /// Распознать моно 16 кГц.
    fn transcribe(&mut self, audio: &PcmAudio, opts: &SttOptions) -> Result<Transcript, SttError>;
    /// Освободить память модели; следующий вызов `transcribe` загрузит её заново.
    fn unload(&mut self);

    /// Определить язык по первым секундам записи, выбирая только среди `opts.allowed_languages`.
    ///
    /// Это отдельный дешёвый шаг, а не режим `auto` у самого распознавания: `auto` заставляет
    /// whisper.cpp считать полное тридцатисекундное окно на каждой реплике. `None` означает, что
    /// движок так не умеет — тогда язык выбирает сама модель во время распознавания.
    fn detect_language(&mut self, _audio: &PcmAudio, _opts: &SttOptions) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_hint_parses_auto_and_codes() {
        assert_eq!(LanguageHint::parse("auto"), LanguageHint::Auto);
        assert_eq!(LanguageHint::parse(""), LanguageHint::Auto);
        assert_eq!(
            LanguageHint::parse(" RU "),
            LanguageHint::Fixed("ru".into())
        );
    }

    #[test]
    fn default_options_allow_russian_and_english() {
        let opts = SttOptions::default();
        assert_eq!(opts.language, LanguageHint::Auto);
        assert_eq!(opts.allowed_languages, vec!["ru", "en"]);
    }
}
