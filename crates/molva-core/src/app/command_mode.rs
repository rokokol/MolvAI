// SPDX-License-Identifier: MIT
//! What command mode tells the model: the system prompt and the request with the instruction
//! and the selected text
//!
//! Both speak the language of the spoken instruction, as the built-in styles speak the language
//! of the phrase: Russian for a Russian instruction, English for any other. A system prompt set
//! in the settings replaces the built-in one for every language

/// The built-in system prompt for a Russian instruction
const SYSTEM_RU: &str = "Ты правишь фрагмент текста по короткой инструкции пользователя. Тебе \
    дают ИНСТРУКЦИЮ и ТЕКСТ. Примени инструкцию к тексту и верни только получившийся текст: без \
    заголовков, без markdown, без кавычек, без пояснений и без слов вроде «вот результат». Если \
    инструкция — перевод, верни только перевод. Не добавляй ничего, чего нет в тексте, сохраняй \
    язык текста, если инструкция не велит его сменить. Если инструкцию выполнить нельзя, верни \
    текст без изменений.";

/// The built-in system prompt for an instruction in any other language
const SYSTEM_EN: &str = "You edit a fragment of text by a short instruction from the user. You \
    get an INSTRUCTION and a TEXT. Apply the instruction to the text and return only the \
    resulting text: no headings, no markdown, no quotes, no explanations and no words such as \
    \"here is the result\". If the instruction is a translation, return only the translation. \
    Add nothing that is not in the text, and keep the language of the text unless the \
    instruction tells you to change it. If you cannot carry out the instruction, return the \
    text unchanged.";

/// The system prompt: the one from the settings when it is set, else the built-in one in `lang`
pub fn system_prompt(configured: &str, lang: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.to_string();
    }
    match lang {
        "ru" => SYSTEM_RU,
        _ => SYSTEM_EN,
    }
    .to_string()
}

/// The request to the model
///
/// The instruction comes first, then the text, so the model does not take a fragment for the
/// task. The reply must be text only, without headings, explanations and quotes
pub fn request(lang: &str, instruction: &str, selection: &str) -> String {
    match lang {
        "ru" => format!(
            "ИНСТРУКЦИЯ: {instruction}\n\nТЕКСТ:\n{selection}\n\n\
             Верни только текст после применения инструкции, одной репликой, без \
             заголовков, пояснений, кавычек и markdown."
        ),
        _ => format!(
            "INSTRUCTION: {instruction}\n\nTEXT:\n{selection}\n\n\
             Return only the text after you apply the instruction, as one reply, without \
             headings, explanations, quotes or markdown."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_cyrillic(text: &str) -> bool {
        text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
    }

    #[test]
    fn the_built_in_prompt_speaks_the_language_of_the_instruction() {
        assert!(has_cyrillic(&system_prompt("", "ru")));
        assert!(!has_cyrillic(&system_prompt("", "en")));
        assert_eq!(system_prompt("", "de"), system_prompt("", "en"));
        assert_eq!(system_prompt("", ""), system_prompt("", "en"));
    }

    #[test]
    fn both_built_in_prompts_demand_the_text_only() {
        assert!(system_prompt("", "ru").contains("только получившийся текст"));
        assert!(system_prompt("", "en").contains("only the resulting text"));
    }

    #[test]
    fn a_prompt_from_the_settings_wins_in_every_language() {
        assert_eq!(system_prompt("Be brief.", "ru"), "Be brief.");
        assert_eq!(system_prompt("Be brief.", "en"), "Be brief.");
        // Whitespace alone is not a prompt: an emptied field must not send an empty instruction
        assert!(has_cyrillic(&system_prompt("  \n", "ru")));
    }

    #[test]
    fn the_request_carries_the_instruction_before_the_text_in_the_phrase_language() {
        let ru = request("ru", "сделай короче", "длинный текст");
        assert!(ru.starts_with("ИНСТРУКЦИЯ: сделай короче"), "{ru}");
        assert!(ru.find("ИНСТРУКЦИЯ") < ru.find("длинный текст"), "{ru}");
        let en = request("en", "make it shorter", "a long text");
        assert!(en.starts_with("INSTRUCTION: make it shorter"), "{en}");
        assert!(en.find("INSTRUCTION") < en.find("a long text"), "{en}");
        assert!(!has_cyrillic(&en), "{en}");
    }
}
