// SPDX-License-Identifier: MIT
//! Translation of one whole text: the prompt, the model call, and what the caller gets instead
//!
//! [`whole_text`] is the only translation path: a dictation after the key release, a file, and an
//! MCP request all give it one finished text and get one answer. The text is never lost: when the
//! model is switched off, privacy forbids the call, the model fails or returns nothing, the
//! original stays and the reason goes beside it for the journal
//!
//! A translation of the chunks while the person is still speaking is a later task: nothing here
//! queues a piece or orders the answers, and a new streaming translator is written apart

use crate::config::LlmConfig;
use crate::domain::llm::{ChatRequest, LlmClient};

use super::llm_output::sanitize_llm_output;

/// Why a text kept the form it was recognized in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Privacy forbids sending text to the model
    Privacy,
    /// The model is switched off in the settings or unavailable
    Unavailable,
    /// The model failed, for example on a timeout or a refused key
    Failed,
    /// The model answered with no text, or with service markup only
    Empty,
}

impl Fallback {
    /// The reason as the journal writes it
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Privacy => "privacy",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Empty => "empty",
        }
    }
}

/// One text: what was recognized, and what the field gets instead
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Piece {
    /// Text before translation: after the dictionary and the rules
    pub raw: String,
    /// What goes to the field: the translation, or `raw` when the text kept its form
    pub text: String,
    /// Target language, ISO 639-1; empty when the text does not translate
    pub target: String,
    /// The model translated this text
    pub translated: bool,
    /// Why the text kept the recognized form; `None` when it does not translate at all
    pub fallback: Option<Fallback>,
}

impl Piece {
    /// A text that does not translate
    pub fn plain(raw: &str) -> Self {
        Self {
            raw: raw.to_string(),
            text: raw.to_string(),
            ..Self::default()
        }
    }

    /// A text the field gets as it was recognized, because the model could not translate it
    pub fn kept(raw: &str, target: &str, fallback: Fallback) -> Self {
        Self {
            raw: raw.to_string(),
            text: raw.to_string(),
            target: target.to_string(),
            translated: false,
            fallback: Some(fallback),
        }
    }
}

/// Why the model cannot be asked at all in this run, or `None` when it can
///
/// The order of the checks is the order of the promise: privacy first, because a switched-off
/// privacy must not be reported as a missing model
pub fn obstacle(llm_enabled: bool, llm_present: bool, send_to_llm: bool) -> Option<Fallback> {
    if !send_to_llm {
        return Some(Fallback::Privacy);
    }
    if !llm_enabled || !llm_present {
        return Some(Fallback::Unavailable);
    }
    None
}

/// Name of the target language for the prompt, from the ISO 639-1 code
///
/// A model follows "into English" more reliably than "into en". An unknown code goes into the
/// prompt as it is: the model reads the code better than nothing
fn language_name(code: &str) -> &str {
    match code {
        "ar" => "Arabic",
        "az" => "Azerbaijani",
        "bg" => "Bulgarian",
        "cs" => "Czech",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "fi" => "Finnish",
        "fr" => "French",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "hu" => "Hungarian",
        "hy" => "Armenian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ka" => "Georgian",
        "kk" => "Kazakh",
        "ko" => "Korean",
        "nl" => "Dutch",
        "no" => "Norwegian",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sr" => "Serbian",
        "sv" => "Swedish",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "uz" => "Uzbek",
        "zh" => "Chinese",
        other => other,
    }
}

/// The system prompt of a translation
///
/// The target language stands in the prompt twice: by name, which the model follows, and by the
/// code from the settings, so the settings and the request cannot drift apart
pub fn system_prompt(target: &str) -> String {
    let code = target.trim().to_lowercase();
    let name = language_name(&code);
    format!(
        "You translate a transcript of speech into {name} (ISO 639-1 code: {code}). \
         Translate the whole text as one coherent passage. Return only the translation: no \
         explanations, no quotes, no markdown, no notes, no transliteration. Keep names, \
         numbers and terms that must not be translated. If the text is already in {name}, \
         return it unchanged."
    )
}

/// The request that translates one text
///
/// Temperature zero: the same text must give the same answer twice, and a translation is not a
/// place for variety. The text goes alone into the message
pub fn request(llm: &LlmConfig, target: &str, text: &str) -> ChatRequest {
    ChatRequest {
        model: llm.model.clone(),
        system: system_prompt(target),
        user: text.to_string(),
        temperature: 0.0,
        max_tokens: llm.max_tokens,
    }
}

/// Translate one whole transcript: dictation after the release, a file, or an MCP request
///
/// One function for every caller. A file or a finished phrase is one text: the answer either
/// replaces it, or the original stays with the reason recorded
pub fn whole_text(llm: &dyn LlmClient, config: &LlmConfig, target: &str, text: &str) -> Piece {
    let text = text.trim();
    let target = target.trim().to_lowercase();
    if text.is_empty() {
        return Piece::plain(text);
    }
    let request = request(config, &target, text);
    match llm.complete(&request) {
        Ok(response) => {
            // Service tags, fences and an introductory phrase are not a translation
            let answer = sanitize_llm_output(&response.text, "");
            if answer.is_empty() {
                return Piece::kept(text, &target, Fallback::Empty);
            }
            Piece {
                raw: text.to_string(),
                text: answer,
                target,
                translated: true,
                fallback: None,
            }
        }
        Err(_) => Piece::kept(text, &target, Fallback::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::FakeLlm;
    use crate::domain::llm::LlmError;

    fn config() -> LlmConfig {
        LlmConfig {
            enabled: true,
            ..LlmConfig::default()
        }
    }

    #[test]
    fn the_prompt_names_the_target_language_and_its_code() {
        let prompt = system_prompt("en");
        assert!(prompt.contains("English"), "{prompt}");
        assert!(prompt.contains("ISO 639-1 code: en"), "{prompt}");
        let ru = system_prompt(" RU ");
        assert!(ru.contains("Russian"), "{ru}");
        assert!(ru.contains("code: ru"), "{ru}");
        // A code the table does not know still reaches the model
        assert!(system_prompt("eo").contains("code: eo"));
    }

    #[test]
    fn the_request_carries_only_the_piece_and_the_target() {
        let llm = LlmConfig {
            model: "qwen3.5:4b".into(),
            ..config()
        };
        let request = request(&llm, "en", "привет мир");
        assert_eq!(request.user, "привет мир");
        assert_eq!(request.model, "qwen3.5:4b");
        assert!(request.system.contains("English"));
        assert_eq!(request.temperature, 0.0);
    }

    #[test]
    fn every_obstacle_has_its_own_name_in_the_journal() {
        assert_eq!(obstacle(true, true, true), None);
        assert_eq!(obstacle(true, true, false), Some(Fallback::Privacy));
        assert_eq!(obstacle(false, true, true), Some(Fallback::Unavailable));
        assert_eq!(obstacle(true, false, true), Some(Fallback::Unavailable));
        assert_eq!(
            obstacle(false, false, false),
            Some(Fallback::Privacy),
            "a phrase that privacy keeps at home is not reported as a missing model"
        );
        assert_eq!(Fallback::Empty.as_str(), "empty");
    }

    #[test]
    fn one_whole_transcript_costs_one_model_call() {
        let llm = FakeLlm::echoing("  \"hello world\"  ");
        let piece = whole_text(&llm, &config(), "EN", "  привет мир  ");
        assert_eq!(piece.raw, "привет мир");
        assert_eq!(piece.text, "hello world");
        assert!(piece.translated);
        assert_eq!(llm.calls(), 1);
        let prompt = llm.last_request.lock().unwrap().clone().unwrap().system;
        assert!(prompt.contains("English"), "{prompt}");
        assert!(prompt.contains("ISO 639-1 code: en"), "{prompt}");
        let empty = whole_text(&llm, &config(), "en", "   ");
        assert!(empty.text.is_empty());
        assert_eq!(llm.calls(), 1, "an empty text must not call the model");
    }

    #[test]
    fn an_empty_answer_keeps_the_recognized_text() {
        // An empty answer is not a translation: the transcript is worth more than an empty
        // string, and the journal learns why
        let llm = FakeLlm::echoing("");
        let piece = whole_text(&llm, &config(), "en", "привет");
        assert_eq!(piece.text, "привет");
        assert!(!piece.translated);
        assert_eq!(piece.fallback, Some(Fallback::Empty));
    }

    #[test]
    fn a_failed_call_keeps_the_recognized_text() {
        let llm = FakeLlm::failing(LlmError::Timeout(5));
        let piece = whole_text(&llm, &config(), "en", "привет");
        assert_eq!(piece.text, "привет");
        assert_eq!(piece.raw, "привет");
        assert!(!piece.translated);
        assert_eq!(piece.fallback, Some(Fallback::Failed));
    }
}
