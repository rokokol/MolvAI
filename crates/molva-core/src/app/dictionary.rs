// SPDX-License-Identifier: MIT
//! Словарь терминов: имена, бренды, идентификаторы кода, которые распознаватель слышит неверно.
//!
//! Файл — TOML:
//!
//! ```toml
//! [[term]]
//! word = "MolvAI"
//! aliases = ["молва", "molvai", "молв ай"]
//! case = "keep"
//! ```
//!
//! Поиск идёт по нормализованной форме (нижний регистр, `ё` как `е`, без крайних знаков), поэтому
//! словарь на 5000 позиций стоит одного обращения к `HashMap` на слово. Нечёткое совпадение
//! включается настройкой и проверяется только против алиасов той же длины ±2 символа — иначе
//! пришлось бы сравнивать каждое слово реплики с каждым алиасом.
//!
//! Многословные алиасы («молв ай») работают: перед точным поиском по одному слову проверяются
//! фразы длиной до `max_alias_words` токенов, длинная фраза выигрывает у короткой.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

use super::rules::{join, normalize, tokenize};

/// Порог нечёткого совпадения: ниже него подстановка чаще вредит, чем помогает.
pub const FUZZY_THRESHOLD: f64 = 0.85;
/// Слова короче этого нечётко не сравниваются: у коротких слов расстояние ничего не значит.
pub const FUZZY_MIN_LEN: usize = 5;
/// Насколько может отличаться длина кандидата при нечётком сравнении.
pub const FUZZY_LEN_WINDOW: usize = 2;
/// Ограничение подсказки для распознавателя: длинный prompt съедает контекст модели.
pub const PROMPT_HINT_MAX_CHARS: usize = 800;

#[derive(Debug, Error)]
pub enum DictionaryError {
    #[error("не удалось прочитать словарь {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ошибка в словаре {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("не удалось записать словарь {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Что делать с регистром подставленного термина.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    /// Как записано в словаре: `MolvAI` остаётся `MolvAI`.
    #[default]
    Keep,
    Upper,
    Lower,
}

/// Одна запись словаря.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub word: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub case: CaseMode,
}

impl Term {
    pub fn new(word: &str, aliases: &[&str]) -> Self {
        Self {
            word: word.to_string(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            case: CaseMode::Keep,
        }
    }

    /// Как термин выглядит в тексте.
    fn rendered(&self) -> String {
        match self.case {
            CaseMode::Keep => self.word.clone(),
            CaseMode::Upper => self.word.to_uppercase(),
            CaseMode::Lower => self.word.to_lowercase(),
        }
    }
}

/// Файл словаря целиком.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DictionaryFile {
    #[serde(default, rename = "term")]
    terms: Vec<Term>,
}

/// Словарь терминов с точным и нечётким поиском.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    terms: Vec<Term>,
    /// Нормализованный алиас (слова через пробел) → индекс термина.
    exact: HashMap<String, usize>,
    /// Длина алиаса в символах → индексы кандидатов для нечёткого поиска.
    by_len: HashMap<usize, Vec<(String, usize)>>,
    max_alias_words: usize,
    fuzzy: bool,
    path: Option<PathBuf>,
    fingerprint: Option<FileFingerprint>,
}

/// Отпечаток файла для горячей перезагрузки: время изменения и размер вместе.
/// Одного mtime мало — на Windows и на файловых системах с грубым временем две записи
/// подряд получают одинаковую метку, и правка словаря остаётся незамеченной.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    modified: SystemTime,
    len: u64,
}

impl Dictionary {
    /// Пустой словарь: реплики проходят через него без изменений.
    pub fn empty() -> Self {
        Self::from_terms(Vec::new(), false)
    }

    /// Словарь из готового списка терминов.
    pub fn from_terms(terms: Vec<Term>, fuzzy: bool) -> Self {
        let mut dictionary = Self {
            terms,
            exact: HashMap::new(),
            by_len: HashMap::new(),
            max_alias_words: 1,
            fuzzy,
            path: None,
            fingerprint: None,
        };
        dictionary.index();
        dictionary
    }

    /// Прочитать словарь из файла. Отсутствующий файл — это пустой словарь, а не ошибка.
    pub fn load(path: &Path, fuzzy: bool) -> Result<Self, DictionaryError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut empty = Self::from_terms(Vec::new(), fuzzy);
                empty.path = Some(path.to_path_buf());
                return Ok(empty);
            }
            Err(source) => {
                return Err(DictionaryError::Read {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let file: DictionaryFile = toml::from_str(&text).map_err(|e| DictionaryError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let mut dictionary = Self::from_terms(file.terms, fuzzy);
        dictionary.path = Some(path.to_path_buf());
        dictionary.fingerprint = file_fingerprint(path);
        Ok(dictionary)
    }

    /// Перечитать файл, если он изменился с прошлой загрузки. `true` — словарь обновился.
    pub fn reload_if_changed(&mut self) -> Result<bool, DictionaryError> {
        let Some(path) = self.path.clone() else {
            return Ok(false);
        };
        let current = file_fingerprint(&path);
        if current == self.fingerprint && self.fingerprint.is_some() {
            return Ok(false);
        }
        let fresh = Self::load(&path, self.fuzzy)?;
        if fresh.terms == self.terms {
            self.fingerprint = current;
            return Ok(false);
        }
        *self = fresh;
        Ok(true)
    }

    /// Дописать термин в файл словаря и переиндексировать.
    pub fn add(&mut self, term: Term) -> Result<(), DictionaryError> {
        self.terms.push(term);
        self.index();
        if let Some(path) = self.path.clone() {
            self.save(&path)?;
        }
        Ok(())
    }

    /// Сохранить словарь в файл.
    pub fn save(&self, path: &Path) -> Result<(), DictionaryError> {
        let file = DictionaryFile {
            terms: self.terms.clone(),
        };
        let text = toml::to_string_pretty(&file).map_err(|e| DictionaryError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| DictionaryError::Write {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
        }
        std::fs::write(path, text).map_err(|source| DictionaryError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn terms(&self) -> &[Term] {
        &self.terms
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn fuzzy(&self) -> bool {
        self.fuzzy
    }

    pub fn set_fuzzy(&mut self, fuzzy: bool) {
        self.fuzzy = fuzzy;
    }

    /// Подсказка для распознавателя: термины через запятую.
    pub fn prompt_hint(&self) -> String {
        let mut hint = String::new();
        for term in &self.terms {
            let candidate = if hint.is_empty() {
                term.word.clone()
            } else {
                format!(", {}", term.word)
            };
            if hint.len() + candidate.len() > PROMPT_HINT_MAX_CHARS {
                break;
            }
            hint.push_str(&candidate);
        }
        hint
    }

    /// Подставить термины. Возвращает текст и число подстановок для поля `dict_hits`.
    pub fn apply(&self, text: &str) -> (String, u32) {
        if self.terms.is_empty() || text.trim().is_empty() {
            return (text.to_string(), 0);
        }
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut hits = 0u32;
        let mut index = 0;
        while index < tokens.len() {
            match self.match_at(&tokens, index) {
                Some((len, term)) => {
                    let tail = trailing_punctuation(&tokens[index + len - 1]);
                    let replacement = format!("{}{tail}", term.rendered());
                    let original: Vec<String> = tokens[index..index + len].to_vec();
                    if join(&original) != replacement {
                        hits += 1;
                    }
                    out.push(replacement);
                    index += len;
                }
                None => {
                    out.push(tokens[index].clone());
                    index += 1;
                }
            }
        }
        (join(&out), hits)
    }

    /// Самое длинное совпадение термина, начинающееся с позиции `at`.
    fn match_at(&self, tokens: &[String], at: usize) -> Option<(usize, &Term)> {
        let max = self.max_alias_words.min(tokens.len() - at);
        for len in (1..=max).rev() {
            let phrase = normalized_phrase(&tokens[at..at + len]);
            if phrase.is_empty() {
                continue;
            }
            if let Some(&term) = self.exact.get(&phrase) {
                return Some((len, &self.terms[term]));
            }
        }
        if !self.fuzzy {
            return None;
        }
        let word = normalize(&tokens[at]);
        self.fuzzy_match(&word).map(|term| (1, &self.terms[term]))
    }

    /// Ближайший алиас среди кандидатов близкой длины.
    fn fuzzy_match(&self, word: &str) -> Option<usize> {
        let len = word.chars().count();
        if len < FUZZY_MIN_LEN {
            return None;
        }
        let mut best: Option<(f64, usize)> = None;
        for candidate_len in len.saturating_sub(FUZZY_LEN_WINDOW)..=len + FUZZY_LEN_WINDOW {
            let Some(candidates) = self.by_len.get(&candidate_len) else {
                continue;
            };
            for (alias, term) in candidates {
                let score = strsim::normalized_levenshtein(word, alias);
                if score >= FUZZY_THRESHOLD && best.map(|(top, _)| score > top).unwrap_or(true) {
                    best = Some((score, *term));
                }
            }
        }
        best.map(|(_, term)| term)
    }

    /// Построить индексы точного и нечёткого поиска.
    fn index(&mut self) {
        self.exact.clear();
        self.by_len.clear();
        self.max_alias_words = 1;
        for (position, term) in self.terms.iter().enumerate() {
            let forms = std::iter::once(&term.word).chain(term.aliases.iter());
            for form in forms {
                let words: Vec<String> = tokenize(form);
                let phrase = normalized_phrase(&words);
                if phrase.is_empty() {
                    warn!(term = %term.word, "алиас без букв и цифр пропущен");
                    continue;
                }
                self.max_alias_words = self.max_alias_words.max(words.len());
                // Первый термин с таким алиасом выигрывает: порядок в файле — воля пользователя.
                self.exact.entry(phrase.clone()).or_insert(position);
                if words.len() == 1 {
                    self.by_len
                        .entry(phrase.chars().count())
                        .or_default()
                        .push((phrase, position));
                }
            }
        }
    }
}

/// Нормализованная фраза: слова через пробел, `ё` как `е`, без крайних знаков.
fn normalized_phrase(words: &[String]) -> String {
    words
        .iter()
        .map(|word| normalize(word))
        .filter(|word| !word.is_empty())
        .collect::<Vec<String>>()
        .join(" ")
}

fn trailing_punctuation(token: &str) -> String {
    token
        .chars()
        .rev()
        .take_while(|c| !c.is_alphanumeric())
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect()
}

fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileFingerprint {
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn molvai() -> Dictionary {
        Dictionary::from_terms(
            vec![
                Term::new("MolvAI", &["молва", "molvai", "молв ай"]),
                Term::new("Hyprland", &["гипрланд", "хипрленд"]),
                Term::new("getUserById", &["гет юзер бай айди"]),
            ],
            false,
        )
    }

    #[test]
    fn exact_alias_is_replaced_ignoring_case() {
        let (text, hits) = molvai().apply("проект Молва растёт");
        assert_eq!(text, "проект MolvAI растёт");
        assert_eq!(hits, 1);
    }

    #[test]
    fn a_multi_word_alias_becomes_one_term() {
        let (text, hits) = molvai().apply("это молв ай");
        assert_eq!(text, "это MolvAI");
        assert_eq!(hits, 1);
        let (text, hits) = molvai().apply("вызови гет юзер бай айди сейчас");
        assert_eq!(text, "вызови getUserById сейчас");
        assert_eq!(hits, 1);
    }

    #[test]
    fn punctuation_after_the_alias_survives() {
        let (text, hits) = molvai().apply("это молва, точно");
        assert_eq!(text, "это MolvAI, точно");
        assert_eq!(hits, 1);
    }

    #[test]
    fn the_term_written_correctly_is_not_counted_as_a_hit() {
        let (text, hits) = molvai().apply("MolvAI уже верно");
        assert_eq!(text, "MolvAI уже верно");
        assert_eq!(hits, 0);
    }

    #[test]
    fn case_from_the_dictionary_wins_over_the_case_in_speech() {
        let (text, _) = molvai().apply("МОЛВА и Гипрланд");
        assert_eq!(text, "MolvAI и Hyprland");
    }

    #[test]
    fn upper_and_lower_case_modes_are_honoured() {
        let dictionary = Dictionary::from_terms(
            vec![
                Term {
                    word: "api".into(),
                    aliases: vec!["апи".into()],
                    case: CaseMode::Upper,
                },
                Term {
                    word: "Nginx".into(),
                    aliases: vec!["энджинкс".into()],
                    case: CaseMode::Lower,
                },
            ],
            false,
        );
        let (text, hits) = dictionary.apply("апи и энджинкс");
        assert_eq!(text, "API и nginx");
        assert_eq!(hits, 2);
    }

    #[test]
    fn fuzzy_matching_catches_a_misheard_ending_only_when_enabled() {
        let mut dictionary = molvai();
        // «гипрланде» — падежная форма, которой нет в списке алиасов.
        assert_eq!(
            dictionary.apply("работает на гипрланде").0,
            "работает на гипрланде"
        );
        dictionary.set_fuzzy(true);
        let (text, hits) = dictionary.apply("работает на гипрланде");
        assert_eq!(text, "работает на Hyprland");
        assert_eq!(hits, 1);
    }

    #[test]
    fn fuzzy_matching_leaves_short_and_distant_words_alone() {
        let mut dictionary = molvai();
        dictionary.set_fuzzy(true);
        // Короткое слово: расстояние в одну букву — это половина слова.
        assert_eq!(dictionary.apply("мол").0, "мол");
        // Далёкое слово не подтягивается.
        assert_eq!(dictionary.apply("документы").0, "документы");
    }

    #[test]
    fn an_empty_dictionary_changes_nothing() {
        let dictionary = Dictionary::empty();
        let (text, hits) = dictionary.apply("любой текст без терминов");
        assert_eq!(text, "любой текст без терминов");
        assert_eq!(hits, 0);
        assert!(dictionary.is_empty());
        assert_eq!(dictionary.prompt_hint(), "");
    }

    #[test]
    fn prompt_hint_lists_terms_and_stays_short() {
        assert_eq!(molvai().prompt_hint(), "MolvAI, Hyprland, getUserById");
        let many: Vec<Term> = (0..500)
            .map(|i| Term::new(&format!("Термин{i}"), &[]))
            .collect();
        let hint = Dictionary::from_terms(many, false).prompt_hint();
        assert!(hint.len() <= PROMPT_HINT_MAX_CHARS, "{}", hint.len());
        assert!(hint.starts_with("Термин0, Термин1"));
    }

    #[test]
    fn a_dictionary_file_is_read_from_toml() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/dictionary.toml");
        let dictionary = Dictionary::load(&path, true).unwrap();
        assert!(dictionary.len() >= 3, "{}", dictionary.len());
        let (text, hits) = dictionary.apply("проект молва работает на гипрланде");
        assert!(text.contains("MolvAI"), "{text}");
        assert!(hits >= 1);
    }

    #[test]
    fn a_missing_file_is_an_empty_dictionary_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let dictionary = Dictionary::load(&dir.path().join("absent.toml"), true).unwrap();
        assert!(dictionary.is_empty());
        assert_eq!(dictionary.apply("текст").0, "текст");
    }

    #[test]
    fn a_broken_file_reports_the_path_and_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictionary.toml");
        std::fs::write(&path, "[[term]]\nword = 12\n").unwrap();
        let err = Dictionary::load(&path, false).unwrap_err().to_string();
        assert!(err.contains("dictionary.toml"), "{err}");
        assert!(err.contains("word"), "{err}");
    }

    #[test]
    fn adding_a_term_writes_the_file_and_takes_effect_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictionary.toml");
        let mut dictionary = Dictionary::load(&path, false).unwrap();
        dictionary
            .add(Term::new("Кубернетес", &["кубер", "k8s"]))
            .unwrap();
        assert_eq!(
            dictionary.apply("развернём кубер").0,
            "развернём Кубернетес"
        );

        let reloaded = Dictionary::load(&path, false).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.apply("развернём k8s").0, "развернём Кубернетес");
    }

    #[test]
    fn reload_picks_up_a_changed_file_and_ignores_an_unchanged_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictionary.toml");
        std::fs::write(
            &path,
            "[[term]]\nword = \"MolvAI\"\naliases = [\"молва\"]\n",
        )
        .unwrap();
        let mut dictionary = Dictionary::load(&path, false).unwrap();
        assert_eq!(dictionary.len(), 1);
        assert!(!dictionary.reload_if_changed().unwrap());

        std::fs::write(
            &path,
            "[[term]]\nword = \"MolvAI\"\naliases = [\"молва\"]\n\
             [[term]]\nword = \"Hyprland\"\naliases = [\"гипрланд\"]\n",
        )
        .unwrap();
        assert!(dictionary.reload_if_changed().unwrap());
        assert_eq!(dictionary.len(), 2);
        assert_eq!(dictionary.apply("гипрланд").0, "Hyprland");
    }

    #[test]
    fn a_change_with_the_same_mtime_is_still_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictionary.toml");
        std::fs::write(
            &path,
            "[[term]]\nword = \"MolvAI\"\naliases = [\"молва\"]\n",
        )
        .unwrap();
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut dictionary = Dictionary::load(&path, false).unwrap();

        // Вторая запись получает ту же метку времени, как на файловых системах с грубым
        // временем: словарь обязан заметить правку по размеру.
        std::fs::write(
            &path,
            "[[term]]\nword = \"MolvAI\"\naliases = [\"молва\"]\n\
             [[term]]\nword = \"Hyprland\"\naliases = [\"гипрланд\"]\n",
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();

        assert!(dictionary.reload_if_changed().unwrap());
        assert_eq!(dictionary.len(), 2);
    }

    #[test]
    fn five_thousand_terms_stay_fast() {
        let terms: Vec<Term> = (0..5000)
            .map(|i| {
                Term::new(
                    &format!("Термин{i}"),
                    &[], // алиасы добавляются ниже, чтобы не держать временные строки
                )
            })
            .map(|mut term| {
                term.aliases = vec![term.word.to_lowercase()];
                term
            })
            .collect();
        let dictionary = Dictionary::from_terms(terms, true);
        let text = "слово ".repeat(200) + "термин4999";
        let started = std::time::Instant::now();
        let (out, hits) = dictionary.apply(&text);
        let elapsed = started.elapsed();
        assert_eq!(hits, 1);
        assert!(out.ends_with("Термин4999"), "{out}");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "словарь на 5000 позиций тормозит: {elapsed:?}"
        );
    }
}
