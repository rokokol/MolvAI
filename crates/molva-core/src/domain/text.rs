// SPDX-License-Identifier: MIT
//! Текст: стили, правила постобработки без модели, подсчёт слов.

use serde::{Deserialize, Serialize};

/// Профиль постобработки. Стиль без модели (`uses_llm = false`) применяет только правила.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub uses_llm: bool,
    pub system_prompt: String,
}

/// Правило преобразования текста без модели: пунктуация словами, «с новой строки», повторы.
pub trait TextRule: std::fmt::Debug + Send + Sync {
    fn id(&self) -> &'static str;
    /// `lang` — код языка реплики (`ru`, `en`), чтобы правило выбирало свой словарь команд.
    fn apply(&self, text: &str, lang: &str) -> String;
}

/// Слово — последовательность непробельных символов, содержащая хотя бы одну букву или цифру.
/// Так «—» и одинокие знаки препинания не считаются словами, а «e-mail» и «2026» считаются.
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
