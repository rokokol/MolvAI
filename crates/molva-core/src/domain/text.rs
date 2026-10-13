// SPDX-License-Identifier: MIT
//! Text: styles, post-processing rules that need no model, word count

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Post-processing profile. A style with no model (`uses_llm = false`) applies only the rules
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub uses_llm: bool,
    /// The prompt for a phrase in any language that `system_prompts` does not name
    pub system_prompt: String,
    /// Prompts for particular phrase languages, by ISO 639-1 code
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub system_prompts: BTreeMap<String, String>,
}

impl Style {
    /// The system prompt for a phrase in `lang`, an ISO 639-1 code or an empty string when the
    /// language is unknown
    ///
    /// A built-in style speaks the language of the phrase it edits: a Russian phrase keeps the
    /// Russian instruction that the styles were tuned with, and an English one gets an English
    /// instruction
    pub fn prompt_for(&self, lang: &str) -> &str {
        self.system_prompts.get(lang).unwrap_or(&self.system_prompt)
    }
}

/// Rule that changes text with no model: punctuation by words, "new line", repeats
pub trait TextRule: std::fmt::Debug + Send + Sync {
    fn id(&self) -> &'static str;
    /// `lang` is the language code of the phrase (`ru`, `en`), so the rule picks its own command
    /// table
    fn apply(&self, text: &str, lang: &str) -> String;
}

/// A word is a sequence of non-whitespace characters with at least one letter or digit.
/// So "—" and lone punctuation marks are not words, but "e-mail" and "2026" are
pub fn word_count(text: &str) -> u32 {
    text.split_whitespace()
        .filter(|token| token.chars().any(char::is_alphanumeric))
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_in_russian_and_english() {
        assert_eq!(word_count("Привет, мир! Hello world"), 4);
    }

    #[test]
    fn ignores_lone_punctuation_and_empty_text() {
        assert_eq!(word_count("— ... , !"), 0);
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   "), 0);
    }

    #[test]
    fn hyphenated_and_numeric_tokens_are_words() {
        assert_eq!(word_count("e-mail 2026 v0.1"), 3);
    }
}
