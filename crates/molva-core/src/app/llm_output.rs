// SPDX-License-Identifier: MIT
//! Чистка ответа модели: в поле ввода попадает текст, а не служебная разметка.
//!
//! Критерий: **служебные теги модели не попадают в текст**. Рассуждающие модели пишут
//! `<think>…</think>`, инструкт-модели оборачивают ответ в ```-ограждения, вежливые модели
//! начинают с «Вот исправленный текст:», а мелкие модели любят вернуть кусок системного промпта
//! или подсказку словаря вместо ответа. Всё это — мусор в поле пользователя, поэтому ответ
//! проходит через [`sanitize_llm_output`] до того, как его увидит вставка.
//!
//! Функция намеренно чистая: разметка сложная, ошибок в ней много, и каждая правка проверяется
//! тестом, а не запуском модели.

/// Теги рассуждений, которые вырезаются вместе с содержимым.
const THINKING_TAGS: [&str; 4] = ["think", "reasoning", "thinking", "thought"];

/// Слова-приметы вводной фразы вида «Вот исправленный текст:».
///
/// Список намеренно узкий: «текст» без «вот» и «исправленный» встречается и в живой речи
/// («Текст доклада: …»), а срезанная реплика — потеря пользователя, в отличие от лишней вводной.
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

/// Максимальная длина вводной фразы: длинная строка с двоеточием — это уже текст пользователя.
const MAX_PREAMBLE_CHARS: usize = 60;

/// Пары кавычек, в которые модель заворачивает ответ целиком.
const QUOTE_PAIRS: [(char, char); 4] = [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\'')];

/// Очистить ответ модели от служебной разметки.
///
/// `dictionary_prompt` — подсказка словаря, которую конвейер отдаёт распознавателю и модели:
/// если модель вернула её эхом вместо ответа, это не текст пользователя.
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

/// Вырезать `<tag>…</tag>` вместе с содержимым; незакрытый тег съедает хвост ответа.
///
/// Незакрытый `<think>` означает, что модель не успела закончить рассуждение: всё после него —
/// обрывок мысли, а не ответ.
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

/// Снять ведущее и замыкающее ```-ограждение.
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines: Vec<&str> = trimmed.lines().collect();
    // Первая строка — само ограждение, возможно с именем языка.
    lines.remove(0);
    if lines.last().map(|line| line.trim_end().ends_with("```")) == Some(true) {
        let last = lines.pop().unwrap_or_default();
        let rest = last.trim_end().trim_end_matches("```");
        if !rest.trim().is_empty() {
            lines.push(rest);
        }
    }
    lines.join("\n").trim().to_string()
}

/// Убрать строки, в которых модель вернула подсказку словаря вместо ответа.
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

/// Ключ для сравнения строк: без регистра, пробелов по краям и хвостовой пунктуации.
fn normalize(line: &str) -> String {
    line.trim()
        .trim_end_matches(['.', ',', ':', ';', '!', '?'])
        .trim()
        .to_lowercase()
}

/// Срезать вводную фразу вроде «Вот исправленный текст:».
fn strip_preamble(text: &str) -> String {
    let text = text.trim_start();
    let Some(colon) = text.find(':') else {
        return text.to_string();
    };
    let head = &text[..colon];
    if head.chars().count() > MAX_PREAMBLE_CHARS || head.contains('\n') {
        return text.to_string();
    }
    // Двоеточие внутри предложения (адреса, время, «итак: » в цитате) — не вводная фраза.
    if head.contains(['.', '!', '?']) {
        return text.to_string();
    }
    let lower = head.to_lowercase();
    if !PREAMBLE_MARKERS.iter().any(|word| lower.contains(word)) {
        return text.to_string();
    }
    let rest = text[colon + ':'.len_utf8()..].trim_start();
    // Вводная фраза без текста после неё — это и есть весь ответ: вернуть нечего.
    rest.to_string()
}

/// Снять кавычки, в которые модель обернула ответ целиком.
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
            // Кавычки внутри текста означают цитату, а не обёртку.
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
            "регистр тега значения не имеет"
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
            "код внутри ограждения остаётся кодом"
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
            "вводная фраза бывает только в начале ответа"
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
            "один только словарь — это не реплика"
        );
        assert_eq!(
            sanitize_llm_output("MolvAI умеет вставлять текст.", hint),
            "MolvAI умеет вставлять текст.",
            "термин внутри реплики остаётся на месте"
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
