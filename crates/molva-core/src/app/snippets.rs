// SPDX-License-Identifier: MIT
//! Exact phrases and wildcard phrases that return a prepared answer before the model runs
//!
//! A snippet matches only the whole phrase, after the dictionary and the rules. Case,
//! punctuation and repeated spaces do not matter: speech recognition writes `Ответь клиенту.`
//! for the phrase `ответь клиенту`, and the person still means the snippet. A phrase that merely
//! contains the trigger does not match, because a snippet replaces the whole text
//!
//! A `*` in the phrase matches any run of characters, including an empty one. So `ответь *`
//! matches `ответь клиенту` and `ответь дорогому клиенту`, and `* клиенту` matches the same
//! phrase from the other side. A wildcard-only phrase is refused: it would replace every
//! dictation. The replacement is static: wildcards change what triggers the snippet, not what
//! the field receives
//!
//! The replacement is final: no style, no model and no rules touch it. A prepared answer is
//! already the text that the field must receive

use crate::config::SnippetsConfig;

use super::rules::{join, normalize, tokenize};

/// Comparison form of a phrase without wildcards: lower case, `ё` as `е`, no punctuation
///
/// Punctuation disappears because the rules add it and the recognizer may hear a comma where
/// the phrase has none. Words and digits stay, and the order stays significant
pub fn key(text: &str) -> String {
    let words: Vec<String> = tokenize(text)
        .iter()
        .map(|word| normalize(word))
        .filter(|word| !word.is_empty())
        .collect();
    join(&words)
}

/// Comparison form of a phrase with wildcards: lower case, `ё` as `е`, no punctuation
///
/// A `*` survives, and the spaces beside it disappear, so `ответь *` and `ответь*` are the same
/// pattern. Both sides use this form, so punctuation never splits a match
fn wildcard_key(text: &str) -> String {
    let lower = text.to_lowercase().replace('ё', "е");
    let mut words = Vec::new();
    let mut word = String::new();
    for ch in lower.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '%' | '*') {
            word.push(ch);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }

    let mut out = String::new();
    for word in words {
        if word == "*" {
            out.push('*');
        } else {
            if !out.is_empty() && !out.ends_with('*') {
                out.push(' ');
            }
            out.push_str(&word);
        }
    }
    out
}

/// Whether a pattern has at least one character that is not a wildcard
fn has_fixed_text(pattern: &str) -> bool {
    pattern.chars().any(|ch| ch != '*')
}

/// Match a wildcard pattern against a normalized text
///
/// `*` matches any run of characters, including none. The table keeps the worst case linear in
/// the product of the lengths, so a pathological phrase cannot turn into an exponential search
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let mut table = vec![vec![false; text.len() + 1]; pattern.len() + 1];
    table[0][0] = true;

    for (row, symbol) in pattern.iter().enumerate() {
        let row = row + 1;
        if *symbol == '*' {
            table[row][0] = table[row - 1][0];
        }
        for column in 1..=text.len() {
            table[row][column] = if *symbol == '*' {
                table[row - 1][column] || table[row][column - 1]
            } else {
                table[row - 1][column - 1] && *symbol == text[column - 1]
            };
        }
    }

    table[pattern.len()][text.len()]
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    Exact(String),
    Wildcard(String),
}

impl Pattern {
    fn from_phrase(phrase: &str) -> Option<Self> {
        if phrase.contains('*') {
            let pattern = wildcard_key(phrase);
            has_fixed_text(&pattern).then_some(Self::Wildcard(pattern))
        } else {
            let key = key(phrase);
            (!key.is_empty()).then_some(Self::Exact(key))
        }
    }

    fn matches(&self, exact: &str, wildcard: &str) -> bool {
        match self {
            Self::Exact(key) => key == exact,
            Self::Wildcard(pattern) => wildcard_matches(pattern, wildcard),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    pattern: Pattern,
    replacement: String,
}

/// Snippets of the current settings, ready for a lookup
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnippetSet {
    enabled: bool,
    entries: Vec<Entry>,
}

impl SnippetSet {
    /// Build from the settings. Invalid entries are skipped here and refused by the validation
    pub fn from_config(config: &SnippetsConfig) -> Self {
        let entries = config
            .items
            .iter()
            .filter_map(|snippet| {
                let pattern = Pattern::from_phrase(&snippet.phrase)?;
                if snippet.replacement.trim().is_empty() {
                    return None;
                }
                Some(Entry {
                    pattern,
                    replacement: snippet.replacement.clone(),
                })
            })
            .collect();
        Self {
            enabled: config.enabled,
            entries,
        }
    }

    /// The replacement for the whole text, or `None` when no snippet matches
    ///
    /// An exact phrase wins over a wildcard one, so a specific answer cannot be hidden by a
    /// broader pattern. Among wildcards the first match in the list wins. The validation
    /// refuses duplicate patterns, so the list order is the order the user sees, not a hidden
    /// priority
    pub fn replacement(&self, text: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        let exact = key(text);
        if exact.is_empty() {
            return None;
        }
        let wildcard = wildcard_key(text);
        self.entries
            .iter()
            .find(|entry| {
                matches!(entry.pattern, Pattern::Exact(_))
                    && entry.pattern.matches(&exact, &wildcard)
            })
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| entry.pattern.matches(&exact, &wildcard))
            })
            .map(|entry| entry.replacement.as_str())
    }
}

/// Identity of a snippet phrase for the duplicate check
///
/// An exact phrase and a wildcard phrase live in different namespaces: `ответь` and `ответь*`
/// are different patterns. An empty result means the phrase has no fixed text and is invalid
pub fn comparison_key(phrase: &str) -> String {
    match Pattern::from_phrase(phrase) {
        Some(Pattern::Exact(key)) => format!("exact:{key}"),
        Some(Pattern::Wildcard(pattern)) => format!("wild:{pattern}"),
        None => String::new(),
    }
}

impl From<&SnippetsConfig> for SnippetSet {
    fn from(config: &SnippetsConfig) -> Self {
        Self::from_config(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Snippet;

    fn snippet(phrase: &str, replacement: &str) -> Snippet {
        Snippet {
            phrase: phrase.into(),
            replacement: replacement.into(),
        }
    }

    fn set(enabled: bool, items: Vec<Snippet>) -> SnippetSet {
        SnippetSet::from_config(&SnippetsConfig { enabled, items })
    }

    #[test]
    fn the_whole_phrase_matches_without_case_and_punctuation() {
        let snippets = set(true, vec![snippet("ответь клиенту", "Добрый день!")]);
        assert_eq!(
            snippets.replacement("Ответь клиенту."),
            Some("Добрый день!")
        );
        assert_eq!(
            snippets.replacement("  ОТВЕТЬ   клиенту  "),
            Some("Добрый день!")
        );
    }

    #[test]
    fn a_phrase_that_only_contains_the_trigger_does_not_match() {
        let snippets = set(true, vec![snippet("ответь клиенту", "Добрый день!")]);
        assert_eq!(snippets.replacement("пожалуйста, ответь клиенту"), None);
        assert_eq!(snippets.replacement("ответь клиенту сейчас"), None);
    }

    #[test]
    fn a_switched_off_list_does_not_match() {
        let snippets = set(false, vec![snippet("ответь клиенту", "Добрый день!")]);
        assert_eq!(snippets.replacement("ответь клиенту"), None);
    }

    #[test]
    fn the_comparison_ignores_commas_and_yo() {
        let snippets = set(true, vec![snippet("всё готово", "Готово.")]);
        assert_eq!(snippets.replacement("Все, готово!"), Some("Готово."));
    }

    #[test]
    fn an_empty_phrase_or_replacement_is_not_a_snippet() {
        let snippets = set(true, vec![snippet("", "answer"), snippet("phrase", "  ")]);
        assert_eq!(snippets.replacement("phrase"), None);
        assert_eq!(snippets.replacement(""), None);
    }

    #[test]
    fn the_replacement_is_returned_as_written() {
        let snippets = set(
            true,
            vec![snippet("ответь клиенту", "  Добрый день!\nС уважением  ")],
        );
        assert_eq!(
            snippets.replacement("ответь клиенту"),
            Some("  Добрый день!\nС уважением  ")
        );
    }

    #[test]
    fn a_wildcard_matches_the_rest_of_the_phrase() {
        let snippets = set(true, vec![snippet("ответь *", "Добрый день!")]);
        assert_eq!(snippets.replacement("ответь клиенту"), Some("Добрый день!"));
        assert_eq!(
            snippets.replacement("ответь дорогому клиенту"),
            Some("Добрый день!")
        );
    }

    #[test]
    fn a_wildcard_matches_nothing_at_all() {
        let snippets = set(true, vec![snippet("ответь *", "Добрый день!")]);
        assert_eq!(snippets.replacement("ответь"), Some("Добрый день!"));
    }

    #[test]
    fn a_wildcard_works_at_the_start_and_in_the_middle() {
        let snippets = set(
            true,
            vec![
                snippet("* клиенту", "first"),
                snippet("ответь * клиенту", "second"),
            ],
        );
        // The first matching wildcard in the list wins
        assert_eq!(
            snippets.replacement("ответь дорогому клиенту"),
            Some("first")
        );
        assert_eq!(snippets.replacement("клиенту"), Some("first"));
    }

    #[test]
    fn an_exact_snippet_wins_over_a_wildcard() {
        let snippets = set(
            true,
            vec![
                snippet("ответь *", "wild"),
                snippet("ответь клиенту", "exact"),
            ],
        );
        assert_eq!(snippets.replacement("ответь клиенту"), Some("exact"));
        assert_eq!(snippets.replacement("ответь другому"), Some("wild"));
    }

    #[test]
    fn a_wildcard_only_phrase_is_not_a_snippet() {
        let snippets = set(true, vec![snippet("*", "everything")]);
        assert_eq!(snippets.replacement("любая фраза"), None);
        assert_eq!(comparison_key("*"), "");
    }

    #[test]
    fn the_wildcard_pattern_ignores_punctuation_and_case() {
        let snippets = set(true, vec![snippet("Ответь *", "ok")]);
        assert_eq!(snippets.replacement("Ответь, дорогой клиент!"), Some("ok"));
    }

    #[test]
    fn the_replacement_does_not_interpolate_the_wildcard() {
        let snippets = set(true, vec![snippet("ответь *", "Шаблон: *")]);
        assert_eq!(snippets.replacement("ответь клиенту"), Some("Шаблон: *"));
    }
}
