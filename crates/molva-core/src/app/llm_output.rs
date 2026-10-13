// SPDX-License-Identifier: MIT
//! Cleanup of the model reply: the input field gets text, not service markup
//!
//! The criterion is that **service tags of the model do not reach the text**. Reasoning models
//! write `<think>…</think>`, instruct models wrap the reply in ``` fences, polite models start
//! with "Вот исправленный текст:", and small models like to return a piece of the system prompt
//! or the dictionary hint instead of the reply. All of this is garbage in the user's field,
//! so the reply goes through [`sanitize_llm_output`] before the paste step sees it
//!
//! The function is pure on purpose: the markup is complex and has many error cases, and a test
//! checks each fix, not a run of the model

/// Reasoning tags that are cut out together with their content
const THINKING_TAGS: [&str; 4] = ["think", "reasoning", "thinking", "thought"];

/// Marker words of an introductory phrase such as "Вот исправленный текст:"
///
/// The list is narrow on purpose: "текст" without "вот" and "исправленный" also occurs in live
/// speech ("Текст доклада: …"), and a phrase that is cut off is a loss for the user, unlike an
/// extra introduction
const PREAMBLE_MARKERS: [&str; 10] = [
    "вот",
    "результат",
    "исправленн",
    "итог",
    "ответ",
    "here is",
    "here's",
    "result",
    "output",
    "corrected",
];

/// Maximum length of an introductory phrase: a long line with a colon is already user text
const MAX_PREAMBLE_CHARS: usize = 60;

/// Pairs of quotes in which the model wraps the whole reply
const QUOTE_PAIRS: [(char, char); 4] = [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\'')];

/// Clean the model reply of service markup
///
/// `dictionary_prompt` is the dictionary hint that the pipeline gives to the recognizer and to
/// the model: if the model returned it as an echo instead of a reply, it is not user text
pub fn sanitize_llm_output(text: &str, dictionary_prompt: &str) -> String {
    let mut out = text.to_string();
    for tag in THINKING_TAGS {
        out = strip_tag_block(&out, tag);
    }
    out = strip_code_fences(&out);
    out = strip_dictionary_echo(&out, dictionary_prompt);
    out = strip_preamble(&out);
    out = unwrap_quotes(&out);
    out.trim().to_string()
}

/// Cut `<tag>…</tag>` together with the content; an unclosed tag eats the tail of the reply
///
/// An unclosed `<think>` means that the model did not finish its reasoning: everything after it
/// is a fragment of a thought, not a reply
fn strip_tag_block(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(start) = lower[cursor..].find(&open) {
        let start = cursor + start;
        out.push_str(&text[cursor..start]);
        match lower[start..].find(&close) {
            Some(end) => cursor = start + end + close.len(),
            None => return out.trim().to_string(),
        }
    }
    out.push_str(&text[cursor..]);
    out.trim().to_string()
}

/// Remove the leading and the trailing ``` fence
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines: Vec<&str> = trimmed.lines().collect();
    // The first line is the fence itself, maybe with a language name
    lines.remove(0);
    if lines
        .last()
        .is_some_and(|line| line.trim_end().ends_with("```"))
    {
        let last = lines.pop().unwrap_or_default();
        let rest = last.trim_end().trim_end_matches("```");
        if !rest.trim().is_empty() {
            lines.push(rest);
        }
    }
    lines.join("\n").trim().to_string()
}

/// Remove the lines in which the model returned the dictionary hint instead of a reply
fn strip_dictionary_echo(text: &str, dictionary_prompt: &str) -> String {
    let prompt = dictionary_prompt.trim();
    if prompt.is_empty() {
        return text.to_string();
    }
    let prompt_key = normalize(prompt);
    if prompt_key.is_empty() {
        return text.to_string();
    }
    text.lines()
        .filter(|line| normalize(line) != prompt_key)
        .collect::<Vec<&str>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Key to compare lines: no case, no edge spaces and no trailing punctuation
fn normalize(line: &str) -> String {
    line.trim()
        .trim_end_matches(['.', ',', ':', ';', '!', '?'])
        .trim()
        .to_lowercase()
}

/// Cut an introductory phrase such as "Вот исправленный текст:"
fn strip_preamble(text: &str) -> String {
    let text = text.trim_start();
    let Some(colon) = text.find(':') else {
        return text.to_string();
    };
    let head = &text[..colon];
    if head.chars().count() > MAX_PREAMBLE_CHARS || head.contains('\n') {
        return text.to_string();
    }
    // A colon inside a sentence (addresses, time, "итак: " in a quote) is not an introduction
    if head.contains(['.', '!', '?']) {
        return text.to_string();
    }
    let lower = head.to_lowercase();
    if !PREAMBLE_MARKERS.iter().any(|word| lower.contains(word)) {
        return text.to_string();
    }
    let rest = text[colon + ':'.len_utf8()..].trim_start();
    // An introduction with no text after it is the whole reply: there is nothing to return
    rest.to_string()
}

/// Remove the quotes in which the model wrapped the whole reply
fn unwrap_quotes(text: &str) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return trimmed.to_string();
    };
    if trimmed.chars().count() < 2 {
        return trimmed.to_string();
    }
    for (open, close) in QUOTE_PAIRS {
        if first == open && last == close {
            let inner = &trimmed[open.len_utf8()..trimmed.len() - close.len_utf8()];
            // Quotes inside the text mean a quotation, not a wrapper
            if !inner.contains(open) && !inner.contains(close) {
                return inner.trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(text: &str) -> String {
        sanitize_llm_output(text, "")
    }

    #[test]
    fn a_thinking_block_never_reaches_the_text() {
        assert_eq!(
            clean("<think>надо переписать вежливее</think>Собрание переносится."),
            "Собрание переносится."
        );
        assert_eq!(
            clean("<reasoning>1) …\n2) …</reasoning>\nГотовый текст."),
            "Готовый текст."
        );
        assert_eq!(
            clean("<THINK>шум</THINK> Текст."),
            "Текст.",
            "the case of the tag does not matter"
        );
    }

    #[test]
    fn an_unclosed_thinking_tag_takes_the_rest_of_the_answer_with_it() {
        assert_eq!(
            clean("Собрание переносится.\n<think>а может быть"),
            "Собрание переносится."
        );
    }

    #[test]
    fn code_fences_are_stripped_from_both_ends() {
        assert_eq!(clean("```\nПривет, мир.\n```"), "Привет, мир.");
        assert_eq!(clean("```text\nПривет, мир.\n```"), "Привет, мир.");
        assert_eq!(
            clean("```\nfn main() {}\n```"),
            "fn main() {}",
            "code inside a fence stays code"
        );
    }

    #[test]
    fn a_fence_inside_the_text_is_left_alone() {
        let text = "Смотри пример:\n```\nfn main() {}\n```\nи всё";
        assert_eq!(clean(text), text);
    }

    #[test]
    fn a_polite_preamble_is_cut_off() {
        assert_eq!(
            clean("Вот исправленный текст: Собрание переносится."),
            "Собрание переносится."
        );
        assert_eq!(clean("Результат:\nСобрание в среду."), "Собрание в среду.");
        assert_eq!(
            clean("Here is the corrected text: The meeting is moved."),
            "The meeting is moved."
        );
    }

    #[test]
    fn a_colon_inside_a_real_sentence_survives() {
        assert_eq!(
            clean("Правило простое: не опаздывать."),
            "Правило простое: не опаздывать."
        );
        assert_eq!(
            clean("Встреча в 10:30 у входа."),
            "Встреча в 10:30 у входа."
        );
        assert_eq!(
            clean("Он сказал так. Вот главное: идём."),
            "Он сказал так. Вот главное: идём.",
            "an introduction occurs only at the start of a reply"
        );
    }

    #[test]
    fn wrapping_quotes_are_removed_but_quoted_speech_is_not() {
        assert_eq!(clean("\"Собрание переносится.\""), "Собрание переносится.");
        assert_eq!(clean("«Собрание переносится.»"), "Собрание переносится.");
        assert_eq!(
            clean("Он сказал: «идём» и ушёл."),
            "Он сказал: «идём» и ушёл."
        );
    }

    #[test]
    fn the_dictionary_hint_echoed_back_is_not_a_reply() {
        let hint = "MolvAI, whisper.cpp, Hyprland";
        assert_eq!(
            sanitize_llm_output(&format!("{hint}\nСобрание переносится."), hint),
            "Собрание переносится."
        );
        assert_eq!(
            sanitize_llm_output(hint, hint),
            "",
            "a dictionary alone is not a phrase"
        );
        assert_eq!(
            sanitize_llm_output("MolvAI умеет вставлять текст.", hint),
            "MolvAI умеет вставлять текст.",
            "a term inside a phrase stays in place"
        );
    }

    #[test]
    fn everything_at_once_still_leaves_only_the_text() {
        let raw =
            "<think>подумаю</think>\n```\nВот исправленный текст: \"Собрание переносится.\"\n```";
        assert_eq!(sanitize_llm_output(raw, "MolvAI"), "Собрание переносится.");
    }

    #[test]
    fn a_clean_answer_is_returned_untouched() {
        assert_eq!(
            clean("Собрание переносится на среду."),
            "Собрание переносится на среду."
        );
        assert_eq!(clean("  Текст с пробелами  "), "Текст с пробелами");
        assert_eq!(clean(""), "");
    }
}
