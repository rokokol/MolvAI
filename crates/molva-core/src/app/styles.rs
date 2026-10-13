// SPDX-License-Identifier: MIT
//! Post-processing styles: built-in profiles and custom ones from the settings
//!
//! A style decides two things: whether a model is needed at all (`uses_llm`) and what to tell
//! the model (`Style::prompt_for`). `verbatim` never calls the model. It is a cheap and
//! predictable mode for passwords, commands and code
//!
//! A built-in style has a Russian prompt for a Russian phrase and an English one for every
//! other phrase. A custom style has one prompt, written by the user, for every language
//!
//! Every system prompt ends with a common condition: keep the input language, add no facts,
//! output only the result. Without it the model readily answers "Конечно! Вот исправленный
//! текст:", and this goes straight into the active field of the user

use crate::config::StyleConfig;
use crate::domain::text::Style;

/// Default style, used when the settings name a style that does not exist
pub const FALLBACK_STYLE: &str = "cleanup";

/// Common tail of the system prompt, the only protection against a talkative model
const COMMON_RULES: &str = " Сохрани язык входного текста. Не добавляй фактов, которых нет в \
    исходном тексте, и ничего не выдумывай. Выведи только результат без пояснений, кавычек и \
    вступлений.";

/// The same tail for the English prompt, which also serves every language without its own
const COMMON_RULES_EN: &str = " Keep the language of the input text. Do not add facts that \
    are not in the source text, and do not invent anything. Output only the result, without \
    explanations, quotes or introductions.";

/// A style that calls the model, with an English prompt and a Russian one
fn model_style(id: &str, name: &str, en: &str, ru: &str) -> Style {
    Style {
        id: id.into(),
        name: name.into(),
        uses_llm: true,
        system_prompt: format!("{en}{COMMON_RULES_EN}"),
        system_prompts: [("ru".to_string(), format!("{ru}{COMMON_RULES}"))].into(),
    }
}

/// Built-in styles, in the order the hotkey cycles through them
fn builtin() -> Vec<Style> {
    vec![
        Style {
            id: "verbatim".into(),
            name: "Verbatim".into(),
            uses_llm: false,
            ..Style::default()
        },
        model_style(
            "cleanup",
            "Cleanup",
            "You edit a transcript of speech. Fix the punctuation, the letter case and obvious \
             recognition errors, and remove slips and filler words (\"um\", \"uh\", \"like\", \
             \"you know\"). Remove nothing else: every other word, including forms of address, \
             names and terms of endearment, must stay in place. Keep the meaning, the order of \
             thoughts and the terminology unchanged.",
            "Ты редактируешь расшифровку устной речи. Исправь пунктуацию, регистр и \
             очевидные ошибки распознавания, убери оговорки и слова-паразиты («ну», «это \
             самое», «как бы», «типа»). Больше не удаляй ничего: каждое остальное слово, \
             включая обращения, имена и ласковые слова, должно остаться на месте. Смысл, \
             порядок мыслей и терминологию сохрани без изменений.",
        ),
        model_style(
            "messenger",
            "Messenger",
            "You turn a transcript of speech into a short messenger message: one to three \
             sentences, a lively conversational tone, no greeting and no signature. Remove \
             repetitions and slips, keep the point.",
            "Ты превращаешь расшифровку речи в короткое сообщение для мессенджера: одна-три \
             фразы, живой разговорный тон, без приветствий и подписи. Убери повторы и \
             оговорки, оставь суть.",
        ),
        model_style(
            "mail",
            "Email",
            "You turn a transcript of speech into a coherent paragraph of a business email: full \
             sentences, a polite neutral tone. Add a greeting and a signature only if they were \
             spoken.",
            "Ты превращаешь расшифровку речи в связный абзац делового письма: полные \
             предложения, вежливый нейтральный тон. Приветствие и подпись добавляй только \
             если они прозвучали.",
        ),
        model_style(
            "code",
            "Code",
            "You write a technical note from a transcript of speech. Keep identifiers, file \
             names, flags, commands and code fragments exactly in the form and letter case in \
             which they were spoken, and do not translate them.",
            "Ты оформляешь техническую заметку по расшифровке речи. Идентификаторы, имена \
             файлов, флаги, команды и фрагменты кода оставляй ровно в том виде и регистре, \
             в каком они произнесены, и не переводи их.",
        ),
        model_style(
            "formal",
            "Formal",
            "You bring a transcript of speech to a formal business style: full sentences, \
             without colloquial phrases, contractions and emotional coloring.",
            "Ты приводишь расшифровку речи к официально-деловому стилю: полные предложения, \
             без разговорных оборотов, сокращений и эмоциональной окраски.",
        ),
    ]
}

/// Set of available styles: the built-in ones plus the custom ones from the settings
#[derive(Debug, Clone)]
pub struct Styles {
    styles: Vec<Style>,
}

impl Default for Styles {
    fn default() -> Self {
        Self { styles: builtin() }
    }
}

impl Styles {
    /// Build the set from the settings. A custom style with the same `id` replaces a built-in one
    pub fn from_config(cfg: &StyleConfig) -> Self {
        let mut styles = builtin();
        for custom in &cfg.custom {
            let style = Style {
                id: custom.id.clone(),
                name: custom.name.clone(),
                uses_llm: custom.uses_llm,
                system_prompt: custom.system_prompt.clone(),
                ..Style::default()
            };
            match styles.iter().position(|s| s.id == style.id) {
                Some(at) => styles[at] = style,
                None => styles.push(style),
            }
        }
        Self { styles }
    }

    pub fn all(&self) -> &[Style] {
        &self.styles
    }

    pub fn get(&self, id: &str) -> Option<&Style> {
        self.styles.iter().find(|style| style.id == id)
    }

    /// Next style in a circle, for the "change style" hotkey
    pub fn next(&self, id: &str) -> &str {
        if self.styles.is_empty() {
            return FALLBACK_STYLE;
        }
        let at = self.styles.iter().position(|style| style.id == id);
        let next = match at {
            Some(at) => (at + 1) % self.styles.len(),
            None => 0,
        };
        &self.styles[next].id
    }

    /// Style for a window class: first the exact rule, then the case-insensitive rule, then the
    /// default
    pub fn for_app(&self, class: Option<&str>, cfg: &StyleConfig) -> String {
        if let Some(class) = class {
            if let Some(style) = cfg.by_app.get(class) {
                return style.clone();
            }
            let lowered = class.to_lowercase();
            for (pattern, style) in &cfg.by_app {
                if pattern.to_lowercase() == lowered {
                    return style.clone();
                }
            }
            // Window classes come in different forms (`org.mozilla.firefox`, `firefox`), so a
            // rule also matches as a substring
            for (pattern, style) in &cfg.by_app {
                let pattern = pattern.to_lowercase();
                if !pattern.is_empty() && lowered.contains(&pattern) {
                    return style.clone();
                }
            }
        }
        cfg.default.clone()
    }

    /// Final style of a phrase: an explicit choice beats the automatic choice by window
    ///
    /// An unknown identifier does not break the pipeline: the style stays `cleanup`
    pub fn resolve(&self, requested: Option<&str>, app: Option<&str>, cfg: &StyleConfig) -> Style {
        let id = match requested {
            Some(id) if self.get(id).is_some() => id.to_string(),
            Some(_) | None => self.for_app(app, cfg),
        };
        self.get(&id)
            .or_else(|| self.get(&cfg.default))
            .or_else(|| self.get(FALLBACK_STYLE))
            .cloned()
            .unwrap_or_else(|| {
                model_style(
                    FALLBACK_STYLE,
                    "Cleanup",
                    "Fix the transcript of speech.",
                    "Исправь расшифровку речи.",
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomStyle;

    #[test]
    fn all_builtin_styles_are_present_and_verbatim_skips_the_model() {
        let styles = Styles::default();
        for id in ["verbatim", "cleanup", "messenger", "mail", "code", "formal"] {
            assert!(styles.get(id).is_some(), "no style {id}");
        }
        assert!(!styles.get("verbatim").unwrap().uses_llm);
        assert!(styles.get("cleanup").unwrap().uses_llm);
    }

    #[test]
    fn every_model_prompt_forbids_inventing_and_demands_the_result_only() {
        for style in Styles::default().all() {
            if !style.uses_llm {
                continue;
            }
            let ru = style.prompt_for("ru");
            assert!(ru.contains("язык входного текста"), "{}", style.id);
            assert!(ru.contains("Не добавляй фактов"), "{}", style.id);
            assert!(ru.contains("только результат"), "{}", style.id);
            let en = style.prompt_for("en");
            assert!(en.contains("language of the input text"), "{}", style.id);
            assert!(en.contains("Do not add facts"), "{}", style.id);
            assert!(en.contains("only the result"), "{}", style.id);
        }
    }

    fn has_cyrillic(text: &str) -> bool {
        text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
    }

    #[test]
    fn a_builtin_prompt_speaks_the_language_of_the_phrase() {
        for style in Styles::default().all() {
            if !style.uses_llm {
                continue;
            }
            assert!(has_cyrillic(style.prompt_for("ru")), "{}", style.id);
            assert!(!has_cyrillic(style.prompt_for("en")), "{}", style.id);
            // Any other language, or none, gets the English prompt, which keeps that language
            assert_eq!(
                style.prompt_for("de"),
                style.prompt_for("en"),
                "{}",
                style.id
            );
            assert_eq!(style.prompt_for(""), style.prompt_for("en"), "{}", style.id);
        }
    }

    #[test]
    fn the_code_style_protects_identifiers() {
        let style = Styles::default().get("code").unwrap().clone();
        assert!(style.prompt_for("ru").contains("Идентификаторы"));
        assert!(style.prompt_for("ru").contains("регистре"));
        assert!(style.prompt_for("en").contains("Keep identifiers"));
        assert!(style.prompt_for("en").contains("letter case"));
    }

    #[test]
    fn the_fallback_style_speaks_the_language_of_the_phrase_too() {
        let style = Styles { styles: Vec::new() }.resolve(None, None, &StyleConfig::default());
        assert!(has_cyrillic(style.prompt_for("ru")));
        assert!(!has_cyrillic(style.prompt_for("en")));
    }

    #[test]
    fn a_custom_prompt_applies_to_every_language() {
        let cfg = StyleConfig {
            custom: vec![CustomStyle {
                id: "poem".into(),
                name: "Poem".into(),
                uses_llm: true,
                system_prompt: "Write in verse.".into(),
            }],
            ..StyleConfig::default()
        };
        let styles = Styles::from_config(&cfg);
        let poem = styles.get("poem").unwrap();
        assert_eq!(poem.prompt_for("ru"), "Write in verse.");
        assert_eq!(poem.prompt_for("en"), "Write in verse.");
    }

    #[test]
    fn a_custom_style_is_added_and_can_replace_a_builtin_one() {
        let cfg = StyleConfig {
            custom: vec![
                CustomStyle {
                    id: "poem".into(),
                    name: "Poem".into(),
                    uses_llm: true,
                    system_prompt: "Write in verse.".into(),
                },
                CustomStyle {
                    id: "cleanup".into(),
                    name: "My cleanup".into(),
                    uses_llm: true,
                    system_prompt: "My prompt.".into(),
                },
            ],
            ..StyleConfig::default()
        };
        let styles = Styles::from_config(&cfg);
        assert_eq!(styles.get("poem").unwrap().name, "Poem");
        // A replaced built-in style loses its Russian prompt too: the user's prompt is the whole style
        assert_eq!(
            styles.get("cleanup").unwrap().prompt_for("ru"),
            "My prompt."
        );
        assert_eq!(styles.all().len(), 7);
    }

    #[test]
    fn next_walks_the_styles_in_a_circle() {
        let styles = Styles::default();
        assert_eq!(styles.next("verbatim"), "cleanup");
        assert_eq!(styles.next("formal"), "verbatim");
        // An unknown style returns to the start of the list
        assert_eq!(styles.next("no such style"), "verbatim");
    }

    #[test]
    fn the_window_class_picks_the_style() {
        let mut by_app = std::collections::BTreeMap::new();
        by_app.insert("kitty".to_string(), "code".to_string());
        by_app.insert("telegram".to_string(), "messenger".to_string());
        let cfg = StyleConfig {
            default: "cleanup".into(),
            by_app,
            custom: Vec::new(),
        };
        let styles = Styles::default();
        assert_eq!(styles.for_app(Some("kitty"), &cfg), "code");
        assert_eq!(styles.for_app(Some("Kitty"), &cfg), "code");
        // The window class is longer than the rule: `org.telegram.desktop` is a messenger too
        assert_eq!(
            styles.for_app(Some("org.telegram.desktop"), &cfg),
            "messenger"
        );
        assert_eq!(styles.for_app(Some("gimp"), &cfg), "cleanup");
        assert_eq!(styles.for_app(None, &cfg), "cleanup");
    }

    #[test]
    fn an_explicit_choice_beats_the_window_rule() {
        let mut by_app = std::collections::BTreeMap::new();
        by_app.insert("kitty".to_string(), "code".to_string());
        let cfg = StyleConfig {
            default: "cleanup".into(),
            by_app,
            custom: Vec::new(),
        };
        let styles = Styles::default();
        assert_eq!(styles.resolve(Some("mail"), Some("kitty"), &cfg).id, "mail");
        assert_eq!(styles.resolve(None, Some("kitty"), &cfg).id, "code");
    }

    #[test]
    fn an_unknown_style_falls_back_instead_of_failing() {
        let cfg = StyleConfig {
            default: "none either".into(),
            ..StyleConfig::default()
        };
        let styles = Styles::default();
        assert_eq!(
            styles.resolve(Some("no such style"), None, &cfg).id,
            "cleanup"
        );
    }
}
