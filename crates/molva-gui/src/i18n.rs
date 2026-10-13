// SPDX-License-Identifier: MIT
//! Interface language for the strings that the Rust side shows: the tray, system notices and
//! command errors
//!
//! The dictionaries are the window's own `ui/src/i18n/*.json`, compiled in, so one string has
//! one translation for the window and the tray. The language is resolved here only: the window
//! asks for it through the `ui_language` command, so the window and the tray cannot disagree

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::LazyLock;

use molva_core::ipc::ErrorCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    /// The code that `ui_language` and the window's dictionaries use
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "ru" => Some(Self::Ru),
            _ => None,
        }
    }
}

type Dictionary = HashMap<String, String>;

// A broken file gives an empty dictionary rather than a panic at start: every string then
// falls back to its key, and a test keeps both files parseable
fn parse(json: &str) -> Dictionary {
    serde_json::from_str(json).unwrap_or_default()
}

static EN: LazyLock<Dictionary> = LazyLock::new(|| parse(include_str!("../ui/src/i18n/en.json")));
static RU: LazyLock<Dictionary> = LazyLock::new(|| parse(include_str!("../ui/src/i18n/ru.json")));

fn dictionary(lang: Lang) -> &'static Dictionary {
    match lang {
        Lang::En => &EN,
        Lang::Ru => &RU,
    }
}

/// The language from `ui_language` when it names a known one, otherwise the system language
///
/// `system` is a BCP 47 tag such as `ru-RU`; an unknown or missing one gives English
pub fn resolve(configured: &str, system: Option<&str>) -> Lang {
    if let Some(lang) = Lang::from_code(configured) {
        return lang;
    }
    // English is the fallback: catalogs and build machines start the app with the C locale
    system
        .and_then(|tag| tag.split(['-', '_']).next())
        .map(str::to_ascii_lowercase)
        .as_deref()
        .and_then(Lang::from_code)
        .unwrap_or(Lang::En)
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Resolve `ui_language` against the system and make it the language of every later string
pub fn apply(configured: &str) {
    let lang = resolve(configured, sys_locale::get_locale().as_deref());
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

/// The string for `key` with each `{name}` replaced from `vars`
///
/// A key missing in the language falls back to English and then to the key itself, so a gap
/// shows at once instead of as an empty label. A missing variable stays in the text
pub fn translate(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    let text = dictionary(lang)
        .get(key)
        .or_else(|| EN.get(key))
        .map_or(key, String::as_str);
    vars.iter().fold(text.to_string(), |text, (name, value)| {
        text.replace(&format!("{{{name}}}"), value)
    })
}

/// The string for `key` in the current language
pub fn t(key: &str) -> String {
    translate(current(), key, &[])
}

/// The string for `key` in the current language, with variables
pub fn tv(key: &str, vars: &[(&str, &str)]) -> String {
    translate(current(), key, vars)
}

/// The daemon sends English only; the window names the kind of failure in its own language
pub fn daemon_error_title(code: ErrorCode) -> String {
    t(match code {
        ErrorCode::Busy => "daemonError.busy",
        ErrorCode::NotRecording => "daemonError.notRecording",
        ErrorCode::NoDevice => "daemonError.noDevice",
        ErrorCode::SttFailed => "daemonError.sttFailed",
        ErrorCode::LlmFailed => "daemonError.llmFailed",
        ErrorCode::InjectFailed => "daemonError.injectFailed",
        ErrorCode::BadRequest => "daemonError.badRequest",
        ErrorCode::Internal => "daemonError.internal",
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn an_explicit_setting_wins_over_the_system() {
        assert_eq!(resolve("ru", Some("en-US")), Lang::Ru);
        assert_eq!(resolve("en", Some("ru-RU")), Lang::En);
    }

    #[test]
    fn auto_follows_the_system_language() {
        assert_eq!(resolve("auto", Some("ru-RU")), Lang::Ru);
        assert_eq!(resolve("auto", Some("ru")), Lang::Ru);
        assert_eq!(resolve("auto", Some("RU_ru")), Lang::Ru);
        assert_eq!(resolve("auto", Some("en-GB")), Lang::En);
    }

    #[test]
    fn an_unknown_or_missing_system_language_gives_english() {
        assert_eq!(resolve("auto", Some("de-DE")), Lang::En);
        assert_eq!(resolve("auto", Some("")), Lang::En);
        assert_eq!(resolve("auto", Some("C")), Lang::En);
        assert_eq!(resolve("auto", None), Lang::En);
    }

    #[test]
    fn an_unknown_setting_behaves_as_auto() {
        assert_eq!(resolve("kz", Some("ru-RU")), Lang::Ru);
        assert_eq!(resolve("kz", Some("fr-FR")), Lang::En);
    }

    #[test]
    fn both_dictionaries_parse_and_have_the_same_keys() {
        let en: BTreeSet<_> = EN.keys().collect();
        let ru: BTreeSet<_> = RU.keys().collect();
        assert!(!en.is_empty());
        let only_en: Vec<_> = en.difference(&ru).collect();
        let only_ru: Vec<_> = ru.difference(&en).collect();
        assert!(only_en.is_empty(), "missing in ru.json: {only_en:?}");
        assert!(only_ru.is_empty(), "missing in en.json: {only_ru:?}");
    }

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .filter(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .collect()
    }

    #[test]
    fn a_translation_keeps_the_variables_of_the_original() {
        for (key, en) in EN.iter() {
            let ru = &RU[key];
            assert_eq!(placeholders(en), placeholders(ru), "{key}");
        }
    }

    /// Every key the Rust sources ask for, found by a call to `t` or `tv` with a literal key
    fn keys_used_by_rust() -> BTreeSet<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut keys = BTreeSet::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for call in ["t(\"", "tv(\""] {
                for (at, _) in source.match_indices(call) {
                    let before = source[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let rest = &source[at + call.len()..];
                    keys.insert(rest[..rest.find('"').unwrap()].to_string());
                }
            }
        }
        keys
    }

    #[test]
    fn every_key_the_rust_side_uses_is_in_the_dictionaries() {
        let used = keys_used_by_rust();
        assert!(
            used.contains("tray.quit"),
            "the scan finds no calls: {used:?}"
        );
        let missing: Vec<_> = used.iter().filter(|key| !EN.contains_key(*key)).collect();
        assert!(missing.is_empty(), "missing in en.json: {missing:?}");
    }

    #[test]
    fn every_daemon_error_code_has_a_title() {
        for code in [
            ErrorCode::Busy,
            ErrorCode::NotRecording,
            ErrorCode::NoDevice,
            ErrorCode::SttFailed,
            ErrorCode::LlmFailed,
            ErrorCode::InjectFailed,
            ErrorCode::BadRequest,
            ErrorCode::Internal,
        ] {
            let title = daemon_error_title(code);
            assert!(!title.starts_with("daemonError."), "{code:?} has no title");
        }
    }

    #[test]
    fn variables_are_substituted_and_a_missing_one_stays_visible() {
        let text = translate(Lang::En, "validation.range", &[("min", "1")]);
        assert!(text.contains('1') && text.contains("{max}"), "{text}");
    }

    #[test]
    fn a_missing_key_shows_itself() {
        assert_eq!(translate(Lang::Ru, "no.such.key", &[]), "no.such.key");
    }

    #[test]
    fn russian_text_comes_from_the_russian_dictionary() {
        assert_eq!(translate(Lang::Ru, "tray.quit", &[]), RU["tray.quit"]);
        assert_ne!(translate(Lang::Ru, "tray.quit", &[]), EN["tray.quit"]);
    }
}
