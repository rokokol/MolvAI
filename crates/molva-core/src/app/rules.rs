// SPDX-License-Identifier: MIT
//! Post-processing rules without a model: spoken punctuation, structure, repeats, numbers, spaces
//!
//! The rules are cheap and predictable, so they always apply, and the model applies only when
//! the style needs it and the phrase is long enough. The order in the set is fixed:
//!
//! 1. `Lists` and `NewLine` are voice commands for structure, and they cut the text into lines;
//! 2. `SpokenPunctuation` turns "запятая" into a separate token `,`;
//! 3. `Glue` puts an email address and a link back together from pieces;
//! 4. `RemoveFillers`, `RemoveRepeats` remove speech garbage;
//! 5. `NumbersAsDigits` turns numerals into digits;
//! 6. `Whitespace` joins punctuation, removes extra spaces and sets non-breaking spaces;
//! 7. `Capitalize` sets a capital letter at the start of a sentence
//!
//! ## Decisions taken
//!
//! - **Repeats.** The same word in a row is removed, except for a short list of intensifiers
//!   (`очень`, `чуть`, `еле`, `very`), where the repeat is part of the meaning, not a speech slip.
//! - **Fillers.** They are removed only as separate words and only if something remains after
//!   that: a phrase of one "ну" must not turn into an empty string.
//! - **Numbers.** A single "один"/"one" does not become a digit, because it is too often a
//!   pronoun. Numerals add up only while the digit position strictly decreases: "три четыре"
//!   stays "3 4".
//! - **Spoken punctuation.** "точка" before "зрения", "отсчёта", "кипения", "опоры" does not
//!   count as a command.
//! - **Dot inside a word.** `example.com` and `whisper.cpp` do not get a space and do not get a
//!   capital letter after the dot: a dot counts as the end of a sentence only before a space or
//!   a capital letter. The word "собака" does not turn into `@`, because the cost of an error is
//!   higher than the benefit.
//! - **Language.** `ru` and `en` take their own tables; an unknown code takes both, because the
//!   language auto-detection may have failed, and the punctuation commands of the two languages
//!   do not overlap

use crate::config::RulesConfig;
use crate::domain::text::TextRule;

/// Units that stick to a number with a non-breaking space
///
/// Single-letter units and units that sound like prepositions ("с", "м", "т", "г") are not
/// included: the cost of a false match is higher than the benefit
const UNITS: &[&str] = &[
    "кг", "мг", "км", "см", "мм", "мл", "л", "га", "шт", "мин", "сек", "%", "₽", "$", "€", "руб",
    "гб", "мб", "кб", "тб", "kg", "km", "cm", "mm", "ml", "gb", "mb", "kb",
];

/// Intensifiers for which a repeat is a device, not a slip
const REPEAT_ALLOWED: &[&str] = &["очень", "чуть", "еле", "very"];

/// Words after which "точка" is part of an expression, not a command
const DOT_IS_NOT_A_COMMAND: &[&str] = &["зрения", "отсчета", "кипения", "опоры", "невозврата"];

/// Split of a text into tokens. Line breaks stay as separate tokens `\n` and `\n\n`
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut newlines = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            newlines += 1;
        } else if ch.is_whitespace() {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
        } else {
            if newlines > 0 {
                tokens.push(newline_token(newlines));
                newlines = 0;
            }
            word.push(ch);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    if newlines > 0 {
        tokens.push(newline_token(newlines));
    }
    tokens
}

fn newline_token(count: usize) -> String {
    if count >= 2 {
        "\n\n".to_string()
    } else {
        "\n".to_string()
    }
}

fn is_newline(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c == '\n')
}

/// Join of tokens back: a space between words, but not around line breaks
pub(crate) fn join(tokens: &[String]) -> String {
    let mut out = String::new();
    for token in tokens {
        if token.is_empty() {
            continue;
        }
        if out.is_empty() || is_newline(token) || out.ends_with('\n') {
            out.push_str(token);
        } else {
            out.push(' ');
            out.push_str(token);
        }
    }
    out
}

/// Form of a word for comparison with the command tables: lower case, no edge marks, `ё` as `е`
pub(crate) fn normalize(token: &str) -> String {
    token
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '%')
        .to_lowercase()
        .replace('ё', "е")
}

/// The longest match of a phrase from the table, starting at position `at`
///
/// Returns the number of consumed tokens and the replacement
fn longest_match<'a>(
    tokens: &[String],
    at: usize,
    table: &[(&'a str, &'a str)],
) -> Option<(usize, &'a str)> {
    let mut best: Option<(usize, &str)> = None;
    for (phrase, replacement) in table {
        let words: Vec<&str> = phrase.split(' ').collect();
        if at + words.len() > tokens.len() {
            continue;
        }
        let matches = words
            .iter()
            .enumerate()
            .all(|(offset, word)| normalize(&tokens[at + offset]) == *word);
        if matches && best.is_none_or(|(len, _)| words.len() > len) {
            best = Some((words.len(), replacement));
        }
    }
    best
}

/// Which command table applies for the language of the phrase
fn tables_for<'a, T>(lang: &str, ru: &'a [T], en: &'a [T]) -> Vec<&'a T> {
    let lang = lang.to_lowercase();
    if lang.starts_with("ru") {
        ru.iter().collect()
    } else if lang.starts_with("en") {
        en.iter().collect()
    } else {
        ru.iter().chain(en.iter()).collect()
    }
}

fn table_for(
    lang: &str,
    ru: &[(&'static str, &'static str)],
    en: &[(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str)> {
    tables_for(lang, ru, en).into_iter().copied().collect()
}

// --- Spoken punctuation -------------------------------------------------------------------

const PUNCTUATION_RU: &[(&str, &str)] = &[
    ("вопросительный знак", "?"),
    ("восклицательный знак", "!"),
    ("знак вопроса", "?"),
    ("знак восклицания", "!"),
    ("точка с запятой", ";"),
    ("кавычки открыть", "«"),
    ("кавычки закрыть", "»"),
    ("открыть кавычки", "«"),
    ("закрыть кавычки", "»"),
    ("открыть скобку", "("),
    ("закрыть скобку", ")"),
    ("скобка открыть", "("),
    ("скобка закрыть", ")"),
    ("многоточие", "…"),
    ("двоеточие", ":"),
    ("запятая", ","),
    ("точка", "."),
    ("тире", "—"),
    ("дефис", "-"),
];

const PUNCTUATION_EN: &[(&str, &str)] = &[
    ("question mark", "?"),
    ("exclamation mark", "!"),
    ("exclamation point", "!"),
    ("full stop", "."),
    ("open quote", "\u{201c}"),
    ("close quote", "\u{201d}"),
    ("open bracket", "("),
    ("close bracket", ")"),
    ("open paren", "("),
    ("close paren", ")"),
    ("ellipsis", "…"),
    ("semicolon", ";"),
    ("colon", ":"),
    ("comma", ","),
    ("period", "."),
    ("dash", "—"),
    ("hyphen", "-"),
];

/// "запятая" → `,`, "вопросительный знак" → `?` and so on
#[derive(Debug, Default, Clone, Copy)]
pub struct SpokenPunctuation;

impl TextRule for SpokenPunctuation {
    fn id(&self) -> &'static str {
        "spoken-punctuation"
    }

    fn apply(&self, text: &str, lang: &str) -> String {
        let table = table_for(lang, PUNCTUATION_RU, PUNCTUATION_EN);
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut index = 0;
        while index < tokens.len() {
            if is_dot_of_a_phrase(&tokens, index) {
                out.push(tokens[index].clone());
                index += 1;
                continue;
            }
            if let Some((len, replacement)) = longest_match(&tokens, index, &table) {
                out.push(replacement.to_string());
                index += len;
            } else {
                out.push(tokens[index].clone());
                index += 1;
            }
        }
        join(&out)
    }
}

fn is_dot_of_a_phrase(tokens: &[String], index: usize) -> bool {
    if normalize(&tokens[index]) != "точка" && normalize(&tokens[index]) != "точки" {
        return false;
    }
    tokens
        .get(index + 1)
        .is_some_and(|next| DOT_IS_NOT_A_COMMAND.contains(&normalize(next).as_str()))
}

// --- Line breaks --------------------------------------------------------------------------

const NEWLINE_RU: &[(&str, &str)] = &[
    ("с новой строки", "\n"),
    ("новая строка", "\n"),
    ("новую строку", "\n"),
    ("с красной строки", "\n\n"),
    ("новый абзац", "\n\n"),
    ("абзац", "\n\n"),
];

const NEWLINE_EN: &[(&str, &str)] = &[
    ("new line", "\n"),
    ("newline", "\n"),
    ("new paragraph", "\n\n"),
    ("paragraph", "\n\n"),
];

/// "с новой строки" → line break, "абзац" → an empty line between paragraphs
#[derive(Debug, Default, Clone, Copy)]
pub struct NewLine;

impl TextRule for NewLine {
    fn id(&self) -> &'static str {
        "new-line"
    }

    fn apply(&self, text: &str, lang: &str) -> String {
        let table = table_for(lang, NEWLINE_RU, NEWLINE_EN);
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut index = 0;
        while index < tokens.len() {
            if let Some((len, replacement)) = longest_match(&tokens, index, &table) {
                out.push(replacement.to_string());
                index += len;
            } else {
                out.push(tokens[index].clone());
                index += 1;
            }
        }
        join(&out)
    }
}

// --- Lists --------------------------------------------------------------------------------

/// List trigger: the phrase, the numbering flag and the separator word of the items
struct ListTrigger {
    phrase: &'static [&'static str],
    numbered: bool,
    separators: &'static [&'static str],
}

const LIST_TRIGGERS: &[ListTrigger] = &[
    ListTrigger {
        phrase: &["маркированный", "список"],
        numbered: false,
        separators: &["пункт", "пункты"],
    },
    ListTrigger {
        phrase: &["нумерованный", "список"],
        numbered: true,
        separators: &["пункт", "пункты"],
    },
    ListTrigger {
        phrase: &["bulleted", "list"],
        numbered: false,
        separators: &["item", "bullet"],
    },
    ListTrigger {
        phrase: &["bullet", "list"],
        numbered: false,
        separators: &["item", "bullet"],
    },
    ListTrigger {
        phrase: &["numbered", "list"],
        numbered: true,
        separators: &["item", "point"],
    },
];

/// "маркированный список пункт молоко пункт хлеб" → a list in lines
///
/// The list runs to the end of the phrase: an explicit command "конец списка" is usually not spoken
#[derive(Debug, Default, Clone, Copy)]
pub struct Lists;

impl TextRule for Lists {
    fn id(&self) -> &'static str {
        "lists"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let tokens = tokenize(text);
        let Some((at, trigger)) = find_trigger(&tokens) else {
            return text.to_string();
        };
        let mut out: Vec<String> = tokens[..at].to_vec();

        let mut items: Vec<Vec<String>> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for token in &tokens[at + trigger.phrase.len()..] {
            if trigger.separators.contains(&normalize(token).as_str()) {
                if !current.is_empty() {
                    items.push(std::mem::take(&mut current));
                }
            } else {
                current.push(token.clone());
            }
        }
        if !current.is_empty() {
            items.push(current);
        }
        if items.is_empty() {
            return text.to_string();
        }

        for (number, item) in items.iter().enumerate() {
            if !out.is_empty() {
                out.push("\n".to_string());
            }
            out.push(if trigger.numbered {
                format!("{}.", number + 1)
            } else {
                "-".to_string()
            });
            out.extend(item.iter().cloned());
        }
        join(&out)
    }
}

fn find_trigger(tokens: &[String]) -> Option<(usize, &'static ListTrigger)> {
    for at in 0..tokens.len() {
        for trigger in LIST_TRIGGERS {
            if at + trigger.phrase.len() > tokens.len() {
                continue;
            }
            let matches = trigger
                .phrase
                .iter()
                .enumerate()
                .all(|(offset, word)| normalize(&tokens[at + offset]) == *word);
            if matches {
                return Some((at, trigger));
            }
        }
    }
    None
}

// --- Addresses and links --------------------------------------------------------------------

/// Schemes after which an address follows, not a new sentence
const URL_STARTS: &[&str] = &["http://", "https://", "www.", "ftp://"];

/// Puts back together the email addresses and links that the recognizer broke with spaces
///
/// The rule is narrow on purpose: it joins only where an `@` or the start of a link is already
/// there. The word "собака" does not turn into `@`: "погладить собака Барсика" costs more than one
/// address
#[derive(Debug, Default, Clone, Copy)]
pub struct Glue;

impl TextRule for Glue {
    fn id(&self) -> &'static str {
        "glue"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            // `name @ domain`: three tokens into one
            if token == "@" && !out.is_empty() && index + 1 < tokens.len() {
                let left = out.last().cloned().unwrap_or_default();
                let right = tokens[index + 1].clone();
                if is_address_part(&left) && is_address_part(&right) {
                    out.pop();
                    out.push(format!("{left}@{right}"));
                    index += 2;
                    continue;
                }
            }
            // `address . ru` or `www . example`: the dot sticks to both sides
            if token == "." && !out.is_empty() && index + 1 < tokens.len() {
                let left = out.last().cloned().unwrap_or_default();
                let right = tokens[index + 1].clone();
                if continues_an_address(&left) && is_address_part(&right) {
                    out.pop();
                    out.push(format!("{left}.{right}"));
                    index += 2;
                    continue;
                }
            }
            // `address .ru`: the dot already grew onto the right piece
            if let Some(rest) = token.strip_prefix('.') {
                let left = out.last().cloned().unwrap_or_default();
                if !out.is_empty() && continues_an_address(&left) && is_address_part(rest) {
                    out.pop();
                    out.push(format!("{left}.{rest}"));
                    index += 1;
                    continue;
                }
            }
            out.push(token.clone());
            index += 1;
        }
        join(&out)
    }
}

/// Piece of an address: letters, digits, hyphens and underscores, with no punctuation
fn is_address_part(token: &str) -> bool {
    !token.is_empty()
        && token.chars().any(char::is_alphanumeric)
        && token
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/' | ':'))
}

/// To the left of the dot there is already an address or a link, not the end of a sentence
fn continues_an_address(token: &str) -> bool {
    if !is_address_part(token) {
        return false;
    }
    let lowered = token.to_lowercase();
    token.contains('@')
        || lowered == "www"
        || URL_STARTS.iter().any(|start| lowered.starts_with(start))
}

// --- Repeats and fillers --------------------------------------------------------------------

/// The same word in a row is kept as one instance
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoveRepeats;

impl TextRule for RemoveRepeats {
    fn id(&self) -> &'static str {
        "remove-repeats"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut previous = String::new();
        for token in tokens {
            let key = normalize(&token);
            let is_word = key.chars().any(char::is_alphanumeric);
            if is_word && key == previous && !REPEAT_ALLOWED.contains(&key.as_str()) {
                continue;
            }
            previous = if is_word { key } else { String::new() };
            out.push(token);
        }
        join(&out)
    }
}

const FILLERS_RU: &[&str] = &[
    "как бы",
    "то есть как бы",
    "ну",
    "типа",
    "короче",
    "вот",
    "значит",
    "э-э",
    "ээ",
    "эм",
    "эээ",
    "мм",
    "ммм",
    "э",
];

const FILLERS_EN: &[&str] = &["you know", "i mean", "um", "uh", "erm", "like"];

/// Filler words are removed, but only if something remains after that
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoveFillers;

impl TextRule for RemoveFillers {
    fn id(&self) -> &'static str {
        "remove-fillers"
    }

    fn apply(&self, text: &str, lang: &str) -> String {
        let fillers: Vec<&&'static str> = tables_for(lang, FILLERS_RU, FILLERS_EN);
        let table: Vec<(&'static str, &'static str)> =
            fillers.into_iter().map(|f| (*f, "")).collect();
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut index = 0;
        while index < tokens.len() {
            if let Some((len, _)) = longest_match(&tokens, index, &table) {
                index += len;
            } else {
                out.push(tokens[index].clone());
                index += 1;
            }
        }
        if out
            .iter()
            .all(|token| !token.chars().any(char::is_alphanumeric))
        {
            // The phrase held only fillers: an empty string is worse than what was said
            return text.to_string();
        }
        join(&out)
    }
}

// --- Numerals -------------------------------------------------------------------------------

/// Value of a numeral and its digit class: the class is needed to avoid adding "три четыре"
struct Numeral {
    value: u64,
    /// 1 is units and teens, 2 is tens, 3 is hundreds; multipliers are handled separately
    class: u8,
    /// Multiplier: "сто" in English and "тысяча" in both languages multiply the accumulated value
    multiplier: bool,
}

fn numeral(word: &str, lang: &str) -> Option<Numeral> {
    let ru = matches!(lang, _ if !lang.to_lowercase().starts_with("en"));
    let en = !lang.to_lowercase().starts_with("ru");
    if ru {
        if let Some(found) = numeral_ru(word) {
            return Some(found);
        }
    }
    if en {
        return numeral_en(word);
    }
    None
}

fn numeral_ru(word: &str) -> Option<Numeral> {
    let unit = |value: u64| {
        Some(Numeral {
            value,
            class: 1,
            multiplier: false,
        })
    };
    let ten = |value: u64| {
        Some(Numeral {
            value,
            class: 2,
            multiplier: false,
        })
    };
    let hundred = |value: u64| {
        Some(Numeral {
            value,
            class: 3,
            multiplier: false,
        })
    };
    match word {
        "ноль" | "нуль" => unit(0),
        "один" | "одна" | "одно" => unit(1),
        "два" | "две" => unit(2),
        "три" => unit(3),
        "четыре" => unit(4),
        "пять" => unit(5),
        "шесть" => unit(6),
        "семь" => unit(7),
        "восемь" => unit(8),
        "девять" => unit(9),
        "десять" => unit(10),
        "одиннадцать" => unit(11),
        "двенадцать" => unit(12),
        "тринадцать" => unit(13),
        "четырнадцать" => unit(14),
        "пятнадцать" => unit(15),
        "шестнадцать" => unit(16),
        "семнадцать" => unit(17),
        "восемнадцать" => unit(18),
        "девятнадцать" => unit(19),
        "двадцать" => ten(20),
        "тридцать" => ten(30),
        "сорок" => ten(40),
        "пятьдесят" => ten(50),
        "шестьдесят" => ten(60),
        "семьдесят" => ten(70),
        "восемьдесят" => ten(80),
        "девяносто" => ten(90),
        "сто" => hundred(100),
        "двести" => hundred(200),
        "триста" => hundred(300),
        "четыреста" => hundred(400),
        "пятьсот" => hundred(500),
        "шестьсот" => hundred(600),
        "семьсот" => hundred(700),
        "восемьсот" => hundred(800),
        "девятьсот" => hundred(900),
        "тысяча" | "тысячи" | "тысяч" => Some(Numeral {
            value: 1000,
            class: 4,
            multiplier: true,
        }),
        _ => None,
    }
}

fn numeral_en(word: &str) -> Option<Numeral> {
    let unit = |value: u64| {
        Some(Numeral {
            value,
            class: 1,
            multiplier: false,
        })
    };
    let ten = |value: u64| {
        Some(Numeral {
            value,
            class: 2,
            multiplier: false,
        })
    };
    match word {
        "zero" => unit(0),
        "one" => unit(1),
        "two" => unit(2),
        "three" => unit(3),
        "four" => unit(4),
        "five" => unit(5),
        "six" => unit(6),
        "seven" => unit(7),
        "eight" => unit(8),
        "nine" => unit(9),
        "ten" => unit(10),
        "eleven" => unit(11),
        "twelve" => unit(12),
        "thirteen" => unit(13),
        "fourteen" => unit(14),
        "fifteen" => unit(15),
        "sixteen" => unit(16),
        "seventeen" => unit(17),
        "eighteen" => unit(18),
        "nineteen" => unit(19),
        "twenty" => ten(20),
        "thirty" => ten(30),
        "forty" => ten(40),
        "fifty" => ten(50),
        "sixty" => ten(60),
        "seventy" => ten(70),
        "eighty" => ten(80),
        "ninety" => ten(90),
        "hundred" => Some(Numeral {
            value: 100,
            class: 3,
            multiplier: true,
        }),
        "thousand" => Some(Numeral {
            value: 1000,
            class: 4,
            multiplier: true,
        }),
        _ => None,
    }
}

/// Single numerals that are more often a pronoun or an article than a number
fn is_lonely_pronoun(word: &str) -> bool {
    matches!(word, "один" | "одна" | "одно" | "одни" | "one")
}

/// Roots of ordinal numerals: "две тысячи двадцать шестого" is not written in digits
const ORDINAL_ROOTS: &[&str] = &[
    "перв",
    "втор",
    "трет",
    "четверт",
    "пят",
    "шест",
    "седьм",
    "восьм",
    "девят",
    "десят",
    "одиннадцат",
    "двенадцат",
    "тринадцат",
    "четырнадцат",
    "пятнадцат",
    "шестнадцат",
    "семнадцат",
    "восемнадцат",
    "девятнадцат",
    "двадцат",
    "тридцат",
    "сороков",
    "сотн",
    "тысячн",
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
];

/// Whether an ordinal numeral follows the chain at once: then the chain must not be touched
fn followed_by_ordinal(tokens: &[String], after: usize) -> bool {
    let Some(next) = tokens.get(after) else {
        return false;
    };
    let next = normalize(next);
    // A numeral itself does not count as ordinal: "двадцать пять" is 25
    if numeral_ru(&next).is_some() || numeral_en(&next).is_some() {
        return false;
    }
    ORDINAL_ROOTS.iter().any(|root| next.starts_with(root))
}

/// "двадцать пять" → "25", "две тысячи двадцать шесть" → "2026"
#[derive(Debug, Default, Clone, Copy)]
pub struct NumbersAsDigits;

impl TextRule for NumbersAsDigits {
    fn id(&self) -> &'static str {
        "numbers-as-digits"
    }

    fn apply(&self, text: &str, lang: &str) -> String {
        let tokens = tokenize(text);
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut index = 0;
        while index < tokens.len() {
            match numeral_run(&tokens, index, lang) {
                Some((len, Some(value))) => {
                    // The trailing punctuation of the last word stays with the number
                    let tail = trailing_punctuation(&tokens[index + len - 1]);
                    out.push(format!("{value}{tail}"));
                    index += len;
                }
                Some((len, None)) => {
                    // The chain is recognized, but it must not be written in digits: we keep the
                    // words
                    out.extend(tokens[index..index + len].iter().cloned());
                    index += len;
                }
                None => {
                    out.push(tokens[index].clone());
                    index += 1;
                }
            }
        }
        join(&out)
    }
}

/// Length of the numeral chain from position `at` and its value
///
/// `Some((len, None))` means there is a chain, but it must not be written in digits (an ordinal
/// follows, or a lonely "один"): such tokens pass on as words
fn numeral_run(tokens: &[String], at: usize, lang: &str) -> Option<(usize, Option<u64>)> {
    let mut total = 0u64;
    let mut current = 0u64;
    let mut last_class = u8::MAX;
    let mut len = 0usize;
    let mut seen = 0usize;

    while at + len < tokens.len() {
        let word = normalize(&tokens[at + len]);
        let Some(found) = numeral(&word, lang) else {
            break;
        };
        if found.multiplier {
            current = current.max(1) * found.value;
            if found.value >= 1000 {
                total += current;
                current = 0;
            }
            last_class = 3;
        } else {
            if found.class >= last_class {
                break;
            }
            current += found.value;
            last_class = found.class;
        }
        len += 1;
        seen += 1;
    }

    if seen == 0 {
        return None;
    }
    if seen == 1 && is_lonely_pronoun(&normalize(&tokens[at])) {
        return None;
    }
    if followed_by_ordinal(tokens, at + len) {
        return Some((len, None));
    }
    Some((len, Some(total + current)))
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

// --- Spaces ---------------------------------------------------------------------------------

/// Extra spaces, a space before punctuation, a non-breaking space in "5 кг"
#[derive(Debug, Default, Clone, Copy)]
pub struct Whitespace {
    /// Non-breaking spaces are set only where the numbers are already written in digits
    pub non_breaking_units: bool,
}

impl TextRule for Whitespace {
    fn id(&self) -> &'static str {
        "whitespace"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let mut tokens = tokenize(text);
        if self.non_breaking_units {
            tokens = glue_units(&tokens);
        }
        tidy(&join(&tokens))
    }
}

fn is_number(token: &str) -> bool {
    let trimmed = token.trim_end_matches([',', '.', ';', ':', '!', '?', ')', '»']);
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
        && trimmed.chars().any(|c| c.is_ascii_digit())
}

/// A number and a unit are joined with a non-breaking space: "5 кг" does not wrap across lines
fn glue_units(tokens: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let next = tokens.get(index + 1);
        let glue = is_number(&tokens[index])
            && next.is_some_and(|unit| UNITS.contains(&normalize(unit).as_str()));
        if glue {
            out.push(format!("{}\u{a0}{}", tokens[index], tokens[index + 1]));
            index += 2;
        } else {
            out.push(tokens[index].clone());
            index += 1;
        }
    }
    out
}

/// Cleanup at the character level: spaces, punctuation, empty lines
fn tidy(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    for (index, &ch) in chars.iter().enumerate() {
        if ch == ' ' {
            // A space before closing punctuation is not needed
            if let Some(next) = chars.get(index + 1) {
                if matches!(
                    next,
                    '.' | ',' | ';' | ':' | '!' | '?' | ')' | '»' | '…' | '\u{201d}'
                ) {
                    continue;
                }
            }
            // Two spaces in a row become one space
            if out.last() == Some(&' ') || out.is_empty() {
                continue;
            }
            // The space after an opening bracket or quote is already eaten below
            if matches!(out.last(), Some('(' | '«' | '\u{201c}')) {
                continue;
            }
        }
        out.push(ch);
    }

    // A space after punctuation, if it was forgotten; decimal fractions are not touched
    let chars = out;
    let mut spaced: Vec<char> = Vec::with_capacity(chars.len() + 8);
    for (index, &ch) in chars.iter().enumerate() {
        spaced.push(ch);
        if !matches!(ch, '.' | ',' | ';' | ':' | '!' | '?') {
            continue;
        }
        let Some(&next) = chars.get(index + 1) else {
            continue;
        };
        if !next.is_alphanumeric() {
            continue;
        }
        let previous = chars.get(index.wrapping_sub(1)).copied().unwrap_or(' ');
        if previous.is_ascii_digit() && next.is_ascii_digit() {
            continue;
        }
        // A dot inside a word is `example.com`, `whisper.cpp` or a version, not the end of a
        // sentence: such a dot must not get a space. The sign of a sentence is a capital letter
        // after it
        if ch == '.' && previous.is_alphanumeric() && !next.is_uppercase() {
            continue;
        }
        spaced.push(' ');
    }

    let text: String = spaced.into_iter().collect();
    // There is never more than one empty line in a row
    let mut result = text.trim().to_string();
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result
        .lines()
        .map(str::trim_end)
        .collect::<Vec<&str>>()
        .join("\n")
}

// --- Capital letters ------------------------------------------------------------------------

/// Capital letter at the start of the text, after a dot and after a line break
///
/// A list marker at the start of a line (`-`, `1.`) neither opens nor closes a sentence: a list
/// item starts with a capital letter in the same way as an ordinary phrase
#[derive(Debug, Default, Clone, Copy)]
pub struct Capitalize;

/// Characters that can stand before the first letter of a sentence
fn opens_a_sentence(ch: char) -> bool {
    matches!(ch, '«' | '"' | '(' | '\u{201c}')
}

/// Marker of a list item at the start of a line
fn is_list_marker(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '-' | '*' | '•' | '.' | ')')
}

/// A dot ends a sentence only if a space or the end of the text follows it
///
/// Otherwise `whisper.cpp` turns into `whisper.Cpp`, and `example.com` into `example.Com`
fn ends_a_sentence(chars: &[char], at: usize) -> bool {
    match chars.get(at + 1) {
        None => true,
        Some(next) => next.is_whitespace() || matches!(next, '"' | '»' | ')' | '\u{201d}'),
    }
}

impl TextRule for Capitalize {
    fn id(&self) -> &'static str {
        "capitalize"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut start_of_sentence = true;
        let mut start_of_line = true;
        for (index, &ch) in chars.iter().enumerate() {
            if start_of_sentence && ch.is_alphabetic() {
                out.extend(ch.to_uppercase());
                start_of_sentence = false;
                start_of_line = false;
                continue;
            }
            if ch == '\n' {
                start_of_sentence = true;
                start_of_line = true;
            } else if matches!(ch, '.' | '!' | '?' | '…')
                && !start_of_line
                && ends_a_sentence(&chars, index)
            {
                start_of_sentence = true;
            } else if !ch.is_whitespace() {
                let keeps = opens_a_sentence(ch) || (start_of_line && is_list_marker(ch));
                if !keeps {
                    start_of_sentence = false;
                    start_of_line = false;
                }
            }
            out.push(ch);
        }
        out
    }
}

// --- Rule set -------------------------------------------------------------------------------

/// Set of rules in a fixed order, built from the settings
pub struct RuleSet {
    rules: Vec<Box<dyn TextRule>>,
}

impl std::fmt::Debug for RuleSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleSet")
            .field("rules", &self.ids())
            .finish()
    }
}

impl RuleSet {
    /// Build the set from the settings. `rules.enabled = false` gives an empty set
    pub fn from_config(cfg: &RulesConfig) -> Self {
        let mut rules: Vec<Box<dyn TextRule>> = Vec::new();
        if !cfg.enabled {
            return Self { rules };
        }
        rules.push(Box::new(Lists));
        rules.push(Box::new(NewLine));
        if cfg.spoken_punctuation {
            rules.push(Box::new(SpokenPunctuation));
        }
        // After punctuation but before the space cleanup: otherwise `Whitespace` splits the
        // address again
        rules.push(Box::new(Glue));
        if cfg.remove_fillers {
            rules.push(Box::new(RemoveFillers));
        }
        if cfg.remove_repeats {
            rules.push(Box::new(RemoveRepeats));
        }
        if cfg.numbers_as_digits {
            rules.push(Box::new(NumbersAsDigits));
        }
        rules.push(Box::new(Whitespace {
            non_breaking_units: cfg.numbers_as_digits,
        }));
        if cfg.auto_punctuation {
            rules.push(Box::new(Capitalize));
        }
        Self { rules }
    }

    /// Identifiers of the rules in the order of application, for diagnostics and logs
    pub fn ids(&self) -> Vec<&'static str> {
        self.rules.iter().map(|rule| rule.id()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Apply the whole set. An empty set returns the text unchanged
    pub fn apply(&self, text: &str, lang: &str) -> String {
        self.rules
            .iter()
            .fold(text.to_string(), |acc, rule| rule.apply(&acc, lang))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> RuleSet {
        RuleSet::from_config(&RulesConfig::default())
    }

    #[test]
    fn spoken_punctuation_becomes_symbols_glued_to_the_word() {
        let out = SpokenPunctuation.apply("привет запятая как дела вопросительный знак", "ru");
        assert_eq!(out, "привет , как дела ?");
        assert_eq!(
            full().apply("привет запятая мир точка", "ru"),
            "Привет, мир."
        );
    }

    #[test]
    fn spoken_punctuation_covers_english() {
        assert_eq!(
            full().apply("hello comma world period", "en"),
            "Hello, world."
        );
        assert_eq!(full().apply("really question mark", "en"), "Really?");
    }

    #[test]
    fn point_of_view_is_not_a_full_stop() {
        assert_eq!(
            full().apply("моя точка зрения другая точка", "ru"),
            "Моя точка зрения другая."
        );
    }

    #[test]
    fn new_line_and_paragraph_commands_break_the_text() {
        assert_eq!(
            full().apply("первая строка с новой строки вторая строка", "ru"),
            "Первая строка\nВторая строка"
        );
        assert_eq!(
            full().apply("вступление абзац продолжение", "ru"),
            "Вступление\n\nПродолжение"
        );
    }

    #[test]
    fn bulleted_and_numbered_lists_are_built_from_the_command() {
        assert_eq!(
            full().apply("купить маркированный список пункт молоко пункт хлеб", "ru"),
            "Купить\n- Молоко\n- Хлеб"
        );
        assert_eq!(
            full().apply("план нумерованный список пункт встреча пункт отчёт", "ru"),
            "План\n1. Встреча\n2. Отчёт"
        );
    }

    #[test]
    fn a_list_command_without_items_leaves_the_text_alone() {
        assert_eq!(
            Lists.apply("маркированный список", "ru"),
            "маркированный список"
        );
    }

    #[test]
    fn repeated_words_collapse_but_intensifiers_survive() {
        assert_eq!(RemoveRepeats.apply("это это важно", "ru"), "это важно");
        assert_eq!(RemoveRepeats.apply("Это это важно", "ru"), "Это важно");
        assert_eq!(
            RemoveRepeats.apply("очень очень важно", "ru"),
            "очень очень важно"
        );
    }

    #[test]
    fn fillers_are_removed_only_as_standalone_words() {
        assert_eq!(
            RemoveFillers.apply("ну как бы это типа работает", "ru"),
            "это работает"
        );
        // "Вот" inside a word is not touched
        assert_eq!(RemoveFillers.apply("вотум доверия", "ru"), "вотум доверия");
        assert_eq!(RemoveFillers.apply("um i think uh so", "en"), "i think so");
    }

    #[test]
    fn an_utterance_made_only_of_fillers_is_kept() {
        assert_eq!(RemoveFillers.apply("ну короче", "ru"), "ну короче");
        // Only punctuation remains of the phrase: this is also an empty phrase
        assert_eq!(RemoveFillers.apply("ну — короче", "ru"), "ну — короче");
    }

    #[test]
    fn numerals_become_digits_when_the_reading_is_unambiguous() {
        assert_eq!(NumbersAsDigits.apply("двадцать пять", "ru"), "25");
        assert_eq!(NumbersAsDigits.apply("сто двадцать три", "ru"), "123");
        assert_eq!(
            NumbersAsDigits.apply("две тысячи двадцать шесть", "ru"),
            "2026"
        );
        assert_eq!(
            NumbersAsDigits.apply("one hundred twenty three", "en"),
            "123"
        );
    }

    #[test]
    fn a_lonely_pronoun_stays_a_word_and_equal_ranks_do_not_add_up() {
        assert_eq!(NumbersAsDigits.apply("один из них", "ru"), "один из них");
        assert_eq!(NumbersAsDigits.apply("три четыре", "ru"), "3 4");
        assert_eq!(NumbersAsDigits.apply("one of them", "en"), "one of them");
    }

    #[test]
    fn an_ordinal_after_the_run_keeps_the_whole_number_in_words() {
        assert_eq!(
            NumbersAsDigits.apply("к две тысячи двадцать шестому году", "ru"),
            "к две тысячи двадцать шестому году"
        );
        assert_eq!(
            NumbersAsDigits.apply("двадцать пятого числа", "ru"),
            "двадцать пятого числа"
        );
        // An ordinary noun after a number does not block the conversion
        assert_eq!(
            NumbersAsDigits.apply("двадцать пять коробок", "ru"),
            "25 коробок"
        );
    }

    #[test]
    fn punctuation_after_a_numeral_survives_the_conversion() {
        assert_eq!(NumbersAsDigits.apply("их двадцать,", "ru"), "их 20,");
    }

    #[test]
    fn extra_spaces_and_spaces_before_punctuation_disappear() {
        let rule = Whitespace::default();
        assert_eq!(
            rule.apply("привет   мир ,  как дела ?", "ru"),
            "привет мир, как дела?"
        );
        assert_eq!(rule.apply("  текст  ", "ru"), "текст");
    }

    #[test]
    fn a_missing_space_after_a_comma_is_added_but_decimals_are_left_alone() {
        let rule = Whitespace::default();
        assert_eq!(rule.apply("привет,мир", "ru"), "привет, мир");
        assert_eq!(rule.apply("цена 1.5 рубля", "ru"), "цена 1.5 рубля");
        assert_eq!(rule.apply("итого 1,5 кг", "ru"), "итого 1,5 кг");
        // A dot between words gets a space only before a capital letter
        assert_eq!(rule.apply("конец.Новое", "ru"), "конец. Новое");
        assert_eq!(rule.apply("собери whisper.cpp", "ru"), "собери whisper.cpp");
    }

    #[test]
    fn units_get_a_non_breaking_space_after_a_number() {
        let rule = Whitespace {
            non_breaking_units: true,
        };
        assert_eq!(rule.apply("5 кг картошки", "ru"), "5\u{a0}кг картошки");
        assert_eq!(rule.apply("рост 10 %", "ru"), "рост 10\u{a0}%");
        // A word that looks like a unit but has no number before it gets an ordinary space
        assert_eq!(rule.apply("просто кг", "ru"), "просто кг");
    }

    #[test]
    fn capitalisation_starts_sentences_and_lines_only() {
        assert_eq!(
            Capitalize.apply("привет. как дела? хорошо", "ru"),
            "Привет. Как дела? Хорошо"
        );
        assert_eq!(Capitalize.apply("строка\nвторая", "ru"), "Строка\nВторая");
        // The case inside words does not change: code identifiers stay as they are
        assert_eq!(
            Capitalize.apply("вызови getUserById потом exit", "ru"),
            "Вызови getUserById потом exit"
        );
    }

    #[test]
    fn a_dot_inside_a_word_does_not_start_a_new_sentence() {
        assert_eq!(
            Capitalize.apply("собери whisper.cpp и открой example.com", "ru"),
            "Собери whisper.cpp и открой example.com"
        );
        assert_eq!(
            Capitalize.apply("готово. следующий шаг", "ru"),
            "Готово. Следующий шаг"
        );
        // A dot at the end of the text does close the sentence
        assert_eq!(Capitalize.apply("готово.", "ru"), "Готово.");
    }

    #[test]
    fn an_email_broken_by_spaces_is_glued_back() {
        assert_eq!(
            Glue.apply("напиши на ivan @ example . com", "ru"),
            "напиши на ivan@example.com"
        );
        assert_eq!(
            full().apply("напиши на ivan @ example точка com", "ru"),
            "Напиши на ivan@example.com"
        );
        assert_eq!(
            full().apply("почта ivan @ example точка com точка", "ru"),
            "Почта ivan@example.com."
        );
        // The word "собака" does not turn into `@`: too often it is the animal
        assert!(!full().apply("погладить собака барсика", "ru").contains('@'));
    }

    #[test]
    fn a_link_broken_by_spaces_is_glued_back() {
        assert_eq!(
            Glue.apply("открой https://example . com", "ru"),
            "открой https://example.com"
        );
        assert_eq!(
            Glue.apply("зайди на www . example . com", "ru"),
            "зайди на www.example.com"
        );
    }

    #[test]
    fn an_ordinary_sentence_is_not_glued_into_an_address() {
        // A dot at the end of a sentence does not glue the next word
        assert_eq!(
            Glue.apply("работа сделана . завтра продолжим", "ru"),
            "работа сделана . завтра продолжим"
        );
        assert_eq!(Glue.apply("почта @ дома", "ru"), "почта@дома");
        assert_eq!(
            full().apply("это конец точка новое начало", "ru"),
            "Это конец. Новое начало"
        );
    }

    #[test]
    fn code_identifiers_survive_the_whole_pipeline() {
        let out = full().apply("вызови getUserById и HTTPServer точка", "ru");
        assert_eq!(out, "Вызови getUserById и HTTPServer.");
    }

    #[test]
    fn disabled_rules_change_nothing() {
        let cfg = RulesConfig {
            enabled: false,
            ..RulesConfig::default()
        };
        let set = RuleSet::from_config(&cfg);
        assert!(set.is_empty());
        assert_eq!(
            set.apply("ну  типа   привет точка", "ru"),
            "ну  типа   привет точка"
        );
    }

    #[test]
    fn each_switch_removes_exactly_its_rule() {
        let cfg = RulesConfig {
            auto_punctuation: false,
            remove_fillers: false,
            ..RulesConfig::default()
        };
        let set = RuleSet::from_config(&cfg);
        assert!(!set.ids().contains(&"capitalize"));
        assert!(!set.ids().contains(&"remove-fillers"));
        assert!(set.ids().contains(&"spoken-punctuation"));
        assert_eq!(set.apply("ну привет точка", "ru"), "ну привет.");
    }

    #[test]
    fn spoken_punctuation_switch_keeps_the_words() {
        let cfg = RulesConfig {
            spoken_punctuation: false,
            ..RulesConfig::default()
        };
        let set = RuleSet::from_config(&cfg);
        assert_eq!(set.apply("привет запятая мир", "ru"), "Привет запятая мир");
    }

    #[test]
    fn empty_text_stays_empty() {
        assert_eq!(full().apply("", "ru"), "");
        assert_eq!(full().apply("   ", "ru"), "");
    }

    #[test]
    fn unknown_language_understands_both_tables() {
        assert_eq!(full().apply("привет точка", "auto"), "Привет.");
        assert_eq!(full().apply("hello period", "auto"), "Hello.");
    }

    // --- Golden tests: real phrases as a whole ------------------------------------------

    fn golden_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/rules")
            .canonicalize()
            .expect("the golden test directory exists")
    }

    /// The language comes from the file name: `ru-punctuation.in.txt` → `ru`
    fn lang_of(name: &str) -> String {
        name.split('-').next().unwrap_or("ru").to_string()
    }

    #[test]
    fn golden_cases_match_the_recorded_output() {
        let directory = golden_dir();
        let update = std::env::var("UPDATE_GOLDEN").is_ok();
        let mut cases: Vec<std::path::PathBuf> = std::fs::read_dir(&directory)
            .expect("the golden directory is readable")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.to_string_lossy().ends_with(".in.txt"))
            .collect();
        cases.sort();
        assert!(!cases.is_empty(), "no golden case in {directory:?}");

        let mut failures = Vec::new();
        for input_path in cases {
            let name = input_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .replace(".in.txt", "");
            let input = std::fs::read_to_string(&input_path).unwrap();
            let actual = full().apply(input.trim_end_matches('\n'), &lang_of(&name));
            let expected_path = input_path.with_file_name(format!("{name}.out.txt"));
            if update {
                std::fs::write(&expected_path, format!("{actual}\n")).unwrap();
                continue;
            }
            let expected = std::fs::read_to_string(&expected_path)
                .unwrap_or_else(|_| panic!("no reference {expected_path:?}; UPDATE_GOLDEN=1"));
            let expected = expected.trim_end_matches('\n');
            if actual != expected {
                failures.push(format!(
                    "{name}:\n  input:    {input:?}\n  expected: {expected:?}\n  actual:   {actual:?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "golden cases differ from the reference:\n{}",
            failures.join("\n")
        );
    }
}
