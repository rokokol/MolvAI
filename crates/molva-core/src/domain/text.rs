// SPDX-License-Identifier: MIT
//! Text: styles, post-processing rules that need no model, word count

use serde::{Deserialize, Serialize};

/// Post-processing profile. A style with no model (`uses_llm = false`) applies only the rules
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub uses_llm: bool,
    pub system_prompt: String,
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
