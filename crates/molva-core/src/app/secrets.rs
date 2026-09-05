// SPDX-License-Identifier: MIT
//! Ключи облачных провайдеров: чтение и маскирование.
//!
//! В файле настроек ключа нет и быть не может — там лежит только имя переменной окружения
//! (`api_key_env`) и источник (`api_key_source`). Источник `keyring` означает хранилище ОС;
//! пока оно не подключено, `keyring` читается из той же переменной окружения и об этом пишется
//! предупреждение, а не тихо возвращается `None`.
//!
//! Всё, что может попасть в лог, оборачивается в [`ApiKey`]: его `Debug` и `Display` печатают
//! маску, поэтому «случайно залогировали структуру целиком» не приводит к утечке.

use std::fmt;

use tracing::warn;

use crate::config::LlmConfig;

/// Сколько символов ключа видно в маске с каждой стороны.
const VISIBLE_HEAD: usize = 3;
const VISIBLE_TAIL: usize = 4;

/// Ключ, который нельзя случайно напечатать.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Явное раскрытие: вызывается ровно там, где ключ уходит в заголовок запроса.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn masked(&self) -> String {
        mask(&self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApiKey({})", self.masked())
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.masked())
    }
}

/// Маска ключа для логов: короткий ключ скрывается целиком.
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= VISIBLE_HEAD + VISIBLE_TAIL {
        return "***".to_string();
    }
    let head: String = chars[..VISIBLE_HEAD].iter().collect();
    let tail: String = chars[chars.len() - VISIBLE_TAIL..].iter().collect();
    format!("{head}…{tail}")
}

/// Ключ для настроенного провайдера; `None` — ключа нет, и это не ошибка для локальных моделей.
pub fn api_key(cfg: &LlmConfig) -> Option<String> {
    api_key_with(cfg, |name| std::env::var(name).ok())
}

/// То же, но с явным источником переменных окружения: тесты не трогают окружение процесса.
pub fn api_key_with<F>(cfg: &LlmConfig, lookup: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let source = cfg.api_key_source.trim().to_lowercase();
    if source == "none" {
        return None;
    }
    if source == "keyring" {
        // Хранилище ОС ещё не подключено; переменная окружения остаётся рабочим путём.
        warn!(
            provider = %cfg.provider,
            "источник ключа keyring пока не реализован, читаю переменную {}",
            cfg.api_key_env
        );
    }
    let name = cfg.api_key_env.trim();
    if name.is_empty() {
        return None;
    }
    let value = lookup(name)?;
    let value = value.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(source: &str, env: &str) -> LlmConfig {
        LlmConfig {
            api_key_source: source.into(),
            api_key_env: env.into(),
            ..LlmConfig::default()
        }
    }

    #[test]
    fn the_key_is_read_from_the_named_environment_variable() {
        let key = api_key_with(&cfg("env", "MOLVA_TEST_KEY"), |name| {
            (name == "MOLVA_TEST_KEY").then(|| "sk-secret-value-1234".to_string())
        });
        assert_eq!(key.as_deref(), Some("sk-secret-value-1234"));
    }

    #[test]
    fn keyring_falls_back_to_the_same_variable_instead_of_failing() {
        let key = api_key_with(&cfg("keyring", "MOLVA_TEST_KEY"), |_| {
            Some("sk-secret-value-1234".into())
        });
        assert_eq!(key.as_deref(), Some("sk-secret-value-1234"));
    }

    #[test]
    fn an_absent_blank_or_disabled_source_gives_no_key() {
        assert_eq!(api_key_with(&cfg("env", "MOLVA_TEST_KEY"), |_| None), None);
        assert_eq!(
            api_key_with(&cfg("env", "MOLVA_TEST_KEY"), |_| Some("   ".into())),
            None
        );
        assert_eq!(api_key_with(&cfg("env", ""), |_| Some("x".into())), None);
        assert_eq!(
            api_key_with(&cfg("none", "MOLVA_TEST_KEY"), |_| Some("x".into())),
            None
        );
    }

    #[test]
    fn a_masked_key_never_contains_the_key() {
        let secret = "sk-proj-0123456789abcdef";
        let masked = mask(secret);
        assert!(!masked.contains(secret), "{masked}");
        assert!(!secret.contains(&masked), "{masked}");
        assert_eq!(masked, "sk-…cdef");
    }

    #[test]
    fn a_short_key_is_hidden_completely() {
        assert_eq!(mask("abc"), "***");
        assert_eq!(mask(""), "***");
        assert_eq!(mask("1234567"), "***");
    }

    #[test]
    fn printing_the_key_prints_the_mask() {
        let secret = "sk-proj-0123456789abcdef";
        let key = ApiKey::new(secret);
        let printed = format!("{key} {key:?}");
        assert!(!printed.contains(secret), "{printed}");
        assert!(!printed.contains("0123456789"), "{printed}");
        assert!(printed.contains("sk-…cdef"), "{printed}");
        assert_eq!(key.expose(), secret);
    }

    #[test]
    fn a_log_line_with_the_key_inside_a_struct_stays_masked() {
        // Поля существуют ровно для того, чтобы попасть в `Debug`: читать их незачем.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Client {
            base_url: String,
            api_key: Option<ApiKey>,
        }
        let secret = "sk-proj-0123456789abcdef";
        let client = Client {
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key: Some(ApiKey::new(secret)),
        };
        let line = format!("{client:?}");
        assert!(!line.contains(secret), "{line}");
        assert!(line.contains("api.groq.com"), "{line}");
    }
}
