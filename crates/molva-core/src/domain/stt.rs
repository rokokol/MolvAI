// SPDX-License-Identifier: MIT
//! Speech recognition: the options, the result and the engine contract

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::audio::PcmAudio;

/// Language hint for the recognizer
///
/// `Fixed` turns off the auto-detection; `Auto` leaves the choice to the model, and the
/// application then checks the result against the list of allowed languages and, if needed,
/// makes one retry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageHint {
    Auto,
    Fixed(String),
}

impl LanguageHint {
    /// Parse the value from the config: `"auto"` or an ISO-639-1 code
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
    /// Languages that the user really uses; with `Auto`, a detection outside the list → retry
    pub allowed_languages: Vec<String>,
    /// Hint for the model: terms, names, place names
    pub initial_prompt: Option<String>,
    /// 0 = all logical cores
    pub threads: usize,
    /// Whether segments with timestamps are needed
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

/// Fragment of recognized text with timestamps relative to the start of the audio
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
    /// Language that the model detected (ISO-639-1), if the engine reports it
    pub detected_language: Option<String>,
    /// Probability that there was no speech: used against hallucinations on silence
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
    #[error("model file not found: {path}. Download it with: molva models pull {model}")]
    ModelMissing { path: String, model: String },
    #[error("could not load the model: {0}")]
    ModelLoad(String),
    #[error("recognition error: {0}")]
    Inference(String),
    #[error("empty audio")]
    EmptyAudio,
    #[error("the engine does not support: {0}")]
    Unsupported(String),
}

/// Recognition engine: whisper.cpp in production, `FakeStt` in tests
pub trait SttEngine: std::fmt::Debug + Send {
    /// Engine identifier for the journal, for example `whisper-cpp`
    fn id(&self) -> &str;
    /// Model name for the journal, for example `small`
    fn model_name(&self) -> &str;
    /// Recognize mono 16 kHz audio
    fn transcribe(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
    ) -> Result<Transcript, SttError>;
    /// Free the model memory; the next `transcribe` call loads it again
    fn unload(&mut self);

    /// Detect the language from the first seconds of the recording, choosing only among `opts.allowed_languages`
    ///
    /// This is a separate cheap step, not the `auto` mode of the recognition itself: `auto` makes
    /// whisper.cpp compute the full thirty-second window on each phrase. `None` means the
    /// engine cannot do this — then the model chooses the language during recognition
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
        let options = SttOptions::default();
        assert_eq!(options.language, LanguageHint::Auto);
        assert_eq!(options.allowed_languages, vec!["ru", "en"]);
    }
}
