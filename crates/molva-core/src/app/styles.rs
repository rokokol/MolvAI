// SPDX-License-Identifier: MIT
//! Стили постобработки: встроенные профили и пользовательские из настроек.
//!
//! Стиль решает две вещи: нужна ли вообще модель (`uses_llm`) и что ей сказать
//! (`system_prompt`). `verbatim` модель не зовёт вовсе — это дешёвый и предсказуемый режим для
//! паролей, команд и кода.
//!
//! Каждый системный промпт заканчивается общим условием: сохранить язык входа, не добавлять
//! фактов, вывести только результат. Без него модель охотно отвечает «Конечно! Вот исправленный
//! текст:» — и это уезжает прямо в активное поле пользователя.

use crate::config::StyleConfig;
use crate::domain::text::Style;

/// Стиль по умолчанию, если в настройках указан несуществующий.
pub const FALLBACK_STYLE: &str = "cleanup";

/// Общий хвост системного промпта — единственная защита от болтливости модели.
const COMMON_RULES: &str = " Сохрани язык входного текста. Не добавляй фактов, которых нет в \
    исходном тексте, и ничего не выдумывай. Выведи только результат без пояснений, кавычек и \
    вступлений.";

/// Встроенные стили в порядке перебора по горячей клавише.
fn builtin() -> Vec<Style> {
    vec![
        Style {
            id: "verbatim".into(),
            name: "Дословно".into(),
            uses_llm: false,
            system_prompt: String::new(),
        },
        Style {
            id: "cleanup".into(),
            name: "Чистка".into(),
            uses_llm: true,
            system_prompt: format!(
                "Ты редактируешь расшифровку устной речи. Исправь пунктуацию, регистр и \
                 очевидные ошибки распознавания, убери оговорки и слова-паразиты. Смысл, \
                 порядок мыслей и терминологию сохрани без изменений.{COMMON_RULES}"
            ),
        },
        Style {
            id: "messenger".into(),
            name: "Мессенджер".into(),
            uses_llm: true,
            system_prompt: format!(
                "Ты превращаешь расшифровку речи в короткое сообщение для мессенджера: одна-три \
                 фразы, живой разговорный тон, без приветствий и подписи. Убери повторы и \
                 оговорки, оставь суть.{COMMON_RULES}"
            ),
        },
        Style {
            id: "mail".into(),
            name: "Почта".into(),
            uses_llm: true,
            system_prompt: format!(
                "Ты превращаешь расшифровку речи в связный абзац делового письма: полные \
                 предложения, вежливый нейтральный тон. Приветствие и подпись добавляй только \
                 если они прозвучали.{COMMON_RULES}"
            ),
        },
        Style {
            id: "code".into(),
            name: "Код".into(),
            uses_llm: true,
            system_prompt: format!(
                "Ты оформляешь техническую заметку по расшифровке речи. Идентификаторы, имена \
                 файлов, флаги, команды и фрагменты кода оставляй ровно в том виде и регистре, \
                 в каком они произнесены, и не переводи их.{COMMON_RULES}"
            ),
        },
        Style {
            id: "formal".into(),
            name: "Официально".into(),
            uses_llm: true,
            system_prompt: format!(
                "Ты приводишь расшифровку речи к официально-деловому стилю: полные предложения, \
                 без разговорных оборотов, сокращений и эмоциональной окраски.{COMMON_RULES}"
            ),
        },
    ]
}

/// Набор доступных стилей: встроенные плюс пользовательские из настроек.
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
    /// Собрать набор по настройкам. Пользовательский стиль с тем же `id` заменяет встроенный.
    pub fn from_config(cfg: &StyleConfig) -> Self {
        let mut styles = builtin();
        for custom in &cfg.custom {
            let style = Style {
                id: custom.id.clone(),
                name: custom.name.clone(),
                uses_llm: custom.uses_llm,
                system_prompt: custom.system_prompt.clone(),
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

    /// Следующий стиль по кругу — для горячей клавиши «сменить стиль».
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

    /// Стиль для класса окна: сначала точное правило, потом регистронезависимое, потом умолчание.
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
            // Классы окон приходят в разном виде (`org.mozilla.firefox`, `firefox`), поэтому
            // правило срабатывает и как подстрока.
            for (pattern, style) in &cfg.by_app {
                let pattern = pattern.to_lowercase();
                if !pattern.is_empty() && lowered.contains(&pattern) {
                    return style.clone();
                }
            }
        }
        cfg.default.clone()
    }

    /// Итоговый стиль реплики: явный выбор важнее автоматики по окну.
    ///
    /// Неизвестный идентификатор не роняет конвейер: остаётся `cleanup`.
    pub fn resolve(&self, requested: Option<&str>, app: Option<&str>, cfg: &StyleConfig) -> Style {
        let id = match requested {
            Some(id) if self.get(id).is_some() => id.to_string(),
            Some(_) | None => self.for_app(app, cfg),
        };
        self.get(&id)
            .or_else(|| self.get(&cfg.default))
            .or_else(|| self.get(FALLBACK_STYLE))
            .cloned()
            .unwrap_or_else(|| Style {
                id: FALLBACK_STYLE.into(),
                name: "Чистка".into(),
                uses_llm: true,
                system_prompt: format!("Исправь расшифровку речи.{COMMON_RULES}"),
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
            assert!(styles.get(id).is_some(), "нет стиля {id}");
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
            let prompt = &style.system_prompt;
            assert!(prompt.contains("язык входного текста"), "{}", style.id);
            assert!(prompt.contains("Не добавляй фактов"), "{}", style.id);
            assert!(prompt.contains("только результат"), "{}", style.id);
        }
    }

    #[test]
    fn the_code_style_protects_identifiers() {
        let style = Styles::default().get("code").unwrap().clone();
        assert!(style.system_prompt.contains("Идентификаторы"));
        assert!(style.system_prompt.contains("регистре"));
    }

    #[test]
    fn a_custom_style_is_added_and_can_replace_a_builtin_one() {
        let cfg = StyleConfig {
            custom: vec![
                CustomStyle {
                    id: "поэма".into(),
                    name: "Поэма".into(),
                    uses_llm: true,
                    system_prompt: "Пиши стихами.".into(),
                },
                CustomStyle {
                    id: "cleanup".into(),
                    name: "Моя чистка".into(),
                    uses_llm: true,
                    system_prompt: "Мой промпт.".into(),
                },
            ],
            ..StyleConfig::default()
        };
        let styles = Styles::from_config(&cfg);
        assert_eq!(styles.get("поэма").unwrap().name, "Поэма");
        assert_eq!(styles.get("cleanup").unwrap().system_prompt, "Мой промпт.");
        assert_eq!(styles.all().len(), 7);
    }

    #[test]
    fn next_walks_the_styles_in_a_circle() {
        let styles = Styles::default();
        assert_eq!(styles.next("verbatim"), "cleanup");
        assert_eq!(styles.next("formal"), "verbatim");
        // Неизвестный стиль возвращает к началу списка.
        assert_eq!(styles.next("нет такого"), "verbatim");
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
        // Класс окна длиннее правила: `org.telegram.desktop` тоже мессенджер.
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
            default: "тоже нет".into(),
            ..StyleConfig::default()
        };
        let styles = Styles::default();
        assert_eq!(styles.resolve(Some("нет такого"), None, &cfg).id, "cleanup");
    }
}
