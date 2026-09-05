// SPDX-License-Identifier: MIT
//! Правила постобработки без модели: пунктуация словами, структура, повторы, числа, пробелы.
//!
//! Правила дёшевы и предсказуемы, поэтому применяются всегда, а модель — только когда стиль
//! этого требует и реплика достаточно длинная. Порядок в наборе фиксирован:
//!
//! 1. `Lists` и `NewLine` — голосовые команды структуры, они режут текст на строки;
//! 2. `SpokenPunctuation` — «запятая» превращается в отдельный токен `,`;
//! 3. `RemoveFillers`, `RemoveRepeats` — мусор речи;
//! 4. `NumbersAsDigits` — числительные в цифры;
//! 5. `Whitespace` — склейка знаков препинания, лишние пробелы, неразрывные пробелы;
//! 6. `Capitalize` — заглавная в начале предложения.
//!
//! ## Принятые решения
//!
//! - **Повторы.** Подряд идущее одинаковое слово снимается, кроме короткого списка усилителей
//!   (`очень`, `чуть`, `еле`, `very`), где повтор — часть смысла, а не сбой речи.
//! - **Заполнители.** Снимаются только как отдельные слова и только если после этого что-то
//!   останется: реплика из одного «ну» превращается в пустую строку не должна.
//! - **Числа.** Одиночное «один»/«one» цифрой не становится — слишком часто это местоимение.
//!   Числительные складываются, только пока разряд строго убывает: «три четыре» остаётся «3 4».
//! - **Пунктуация словами.** «точка» перед «зрения», «отсчёта», «кипения», «опоры» не считается
//!   командой.
//! - **Язык.** `ru` и `en` берут свои таблицы; неизвестный код — обе, потому что автоопределение
//!   языка могло не сработать, а команды пунктуации в двух языках не пересекаются.

use crate::config::RulesConfig;
use crate::domain::text::TextRule;

/// Единицы, которые прилипают к числу неразрывным пробелом.
///
/// Однобуквенные и омонимичные предлогам («с», «м», «т», «г») сюда не входят: цена ложного
/// срабатывания выше пользы.
const UNITS: &[&str] = &[
    "кг", "мг", "км", "см", "мм", "мл", "л", "га", "шт", "мин", "сек", "%", "₽", "$", "€", "руб",
    "гб", "мб", "кб", "тб", "kg", "km", "cm", "mm", "ml", "gb", "mb", "kb",
];

/// Усилители, для которых повтор — приём, а не сбой.
const REPEAT_ALLOWED: &[&str] = &["очень", "чуть", "еле", "very"];

/// Слова, после которых «точка» — часть выражения, а не команда.
const DOT_IS_NOT_A_COMMAND: &[&str] = &["зрения", "отсчета", "кипения", "опоры", "невозврата"];

/// Разбор текста на токены. Переводы строк остаются отдельными токенами `\n` и `\n\n`.
fn tokenize(text: &str) -> Vec<String> {
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

/// Сборка токенов обратно: пробел между словами, но не вокруг переводов строки.
fn join(tokens: &[String]) -> String {
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

/// Форма слова для сравнения со словарями команд: нижний регистр, без крайних знаков, `ё` как `е`.
fn normalize(token: &str) -> String {
    token
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '%')
        .to_lowercase()
        .replace('ё', "е")
}

/// Самое длинное совпадение фразы из таблицы, начиная с позиции `at`.
///
/// Возвращает число съеденных токенов и замену.
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
        if matches && best.map(|(len, _)| words.len() > len).unwrap_or(true) {
            best = Some((words.len(), replacement));
        }
    }
    best
}

/// Какая таблица команд действует для языка реплики.
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

// --- Пунктуация словами -------------------------------------------------------------------

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

/// «запятая» → `,`, «вопросительный знак» → `?` и так далее.
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
            match longest_match(&tokens, index, &table) {
                Some((len, replacement)) => {
                    out.push(replacement.to_string());
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

fn is_dot_of_a_phrase(tokens: &[String], index: usize) -> bool {
    if normalize(&tokens[index]) != "точка" && normalize(&tokens[index]) != "точки" {
        return false;
    }
    tokens
        .get(index + 1)
        .map(|next| DOT_IS_NOT_A_COMMAND.contains(&normalize(next).as_str()))
        .unwrap_or(false)
}

// --- Переводы строки ----------------------------------------------------------------------

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

/// «с новой строки» → перевод строки, «абзац» → пустая строка между абзацами.
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
            match longest_match(&tokens, index, &table) {
                Some((len, replacement)) => {
                    out.push(replacement.to_string());
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

// --- Списки -------------------------------------------------------------------------------

/// Триггер списка: фраза, флаг нумерации и слово-разделитель пунктов.
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

/// «маркированный список пункт молоко пункт хлеб» → список строками.
///
/// Список тянется до конца реплики: явной команды «конец списка» в речи обычно не звучит.
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

// --- Повторы и заполнители ------------------------------------------------------------------

/// Подряд идущее одинаковое слово оставляется в одном экземпляре.
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

/// Слова-заполнители снимаются, но только если после этого что-то останется.
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
            match longest_match(&tokens, index, &table) {
                Some((len, _)) => index += len,
                None => {
                    out.push(tokens[index].clone());
                    index += 1;
                }
            }
        }
        if out
            .iter()
            .all(|token| !token.chars().any(char::is_alphanumeric))
        {
            // Реплика состояла из одних заполнителей: пустая строка хуже, чем сказанное.
            return text.to_string();
        }
        join(&out)
    }
}

// --- Числительные ---------------------------------------------------------------------------

/// Значение числительного и его разряд: разряд нужен, чтобы не складывать «три четыре».
struct Numeral {
    value: u64,
    /// 1 — единицы и подростковые, 2 — десятки, 3 — сотни; множители обрабатываются отдельно.
    class: u8,
    /// Множитель: «сто» в английском и «тысяча» в обоих языках умножают накопленное.
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

/// Одиночные числительные, которые чаще местоимение или артикль, чем число.
fn is_lonely_pronoun(word: &str) -> bool {
    matches!(word, "один" | "одна" | "одно" | "одни" | "one")
}

/// Корни порядковых числительных: «две тысячи двадцать шестого» цифрами не записывается.
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

/// Стоит ли сразу за цепочкой порядковое числительное: тогда цепочку трогать нельзя.
fn followed_by_ordinal(tokens: &[String], after: usize) -> bool {
    let Some(next) = tokens.get(after) else {
        return false;
    };
    let next = normalize(next);
    // Само числительное порядковым не считается: «двадцать пять» — это 25.
    if numeral_ru(&next).is_some() || numeral_en(&next).is_some() {
        return false;
    }
    ORDINAL_ROOTS.iter().any(|root| next.starts_with(root))
}

/// «двадцать пять» → «25», «две тысячи двадцать шесть» → «2026».
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
                    // Хвост знаков препинания у последнего слова остаётся при числе.
                    let tail = trailing_punctuation(&tokens[index + len - 1]);
                    out.push(format!("{value}{tail}"));
                    index += len;
                }
                Some((len, None)) => {
                    // Цепочка распознана, но записывать её цифрами нельзя: оставляем словами.
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

/// Длина цепочки числительных с позиции `at` и её значение.
///
/// `Some((len, None))` — цепочка есть, но записывать её цифрами нельзя (порядковое следом,
/// одинокое «один»): такие токены проходят дальше словами.
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

// --- Пробелы --------------------------------------------------------------------------------

/// Лишние пробелы, пробел перед знаком препинания, неразрывный пробел в «5 кг».
#[derive(Debug, Default, Clone, Copy)]
pub struct Whitespace {
    /// Неразрывные пробелы ставятся только там, где числа уже записаны цифрами.
    pub non_breaking_units: bool,
}

impl TextRule for Whitespace {
    fn id(&self) -> &'static str {
        "whitespace"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let mut tokens = tokenize(text);
        if self.non_breaking_units {
            tokens = glue_units(tokens);
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

/// Число и единица склеиваются неразрывным пробелом: «5 кг» не переносится по строкам.
fn glue_units(tokens: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let next = tokens.get(index + 1);
        let glue = is_number(&tokens[index])
            && next
                .map(|unit| UNITS.contains(&normalize(unit).as_str()))
                .unwrap_or(false);
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

/// Чистка на уровне символов: пробелы, знаки препинания, пустые строки.
fn tidy(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    for (index, &ch) in chars.iter().enumerate() {
        if ch == ' ' {
            // Пробел перед закрывающим знаком препинания не нужен.
            if let Some(next) = chars.get(index + 1) {
                if matches!(
                    next,
                    '.' | ',' | ';' | ':' | '!' | '?' | ')' | '»' | '…' | '\u{201d}'
                ) {
                    continue;
                }
            }
            // Два пробела подряд — один пробел.
            if out.last() == Some(&' ') || out.is_empty() {
                continue;
            }
            // Пробел после открывающей скобки или кавычки уже съеден ниже.
            if matches!(out.last(), Some('(') | Some('«') | Some('\u{201c}')) {
                continue;
            }
        }
        out.push(ch);
    }

    // Пробел после знака препинания, если его забыли; десятичные дроби не трогаем.
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
        spaced.push(' ');
    }

    let text: String = spaced.into_iter().collect();
    // Больше одной пустой строки подряд не бывает.
    let mut result = text.trim().to_string();
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<&str>>()
        .join("\n")
}

// --- Заглавные ------------------------------------------------------------------------------

/// Заглавная буква в начале текста, после точки и после перевода строки.
///
/// Маркер списка в начале строки (`-`, `1.`) предложение не открывает и не закрывает: пункт
/// списка начинается с заглавной так же, как обычная фраза.
#[derive(Debug, Default, Clone, Copy)]
pub struct Capitalize;

/// Символы, которые могут стоять перед первой буквой предложения.
fn opens_a_sentence(ch: char) -> bool {
    matches!(ch, '«' | '"' | '(' | '\u{201c}')
}

/// Маркер пункта списка в начале строки.
fn is_list_marker(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '-' | '*' | '•' | '.' | ')')
}

impl TextRule for Capitalize {
    fn id(&self) -> &'static str {
        "capitalize"
    }

    fn apply(&self, text: &str, _lang: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut start_of_sentence = true;
        let mut start_of_line = true;
        for ch in text.chars() {
            if start_of_sentence && ch.is_alphabetic() {
                out.extend(ch.to_uppercase());
                start_of_sentence = false;
                start_of_line = false;
                continue;
            }
            if ch == '\n' {
                start_of_sentence = true;
                start_of_line = true;
            } else if matches!(ch, '.' | '!' | '?' | '…') && !start_of_line {
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

// --- Набор правил ---------------------------------------------------------------------------

/// Набор правил в фиксированном порядке, собранный по настройкам.
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
    /// Собрать набор по настройкам. `rules.enabled = false` даёт пустой набор.
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

    /// Идентификаторы правил в порядке применения — для диагностики и логов.
    pub fn ids(&self) -> Vec<&'static str> {
        self.rules.iter().map(|rule| rule.id()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Применить набор целиком. Пустой набор возвращает текст без изменений.
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
        // «Вот» внутри слова не трогается.
        assert_eq!(RemoveFillers.apply("вотум доверия", "ru"), "вотум доверия");
        assert_eq!(RemoveFillers.apply("um i think uh so", "en"), "i think so");
    }

    #[test]
    fn an_utterance_made_only_of_fillers_is_kept() {
        assert_eq!(RemoveFillers.apply("ну короче", "ru"), "ну короче");
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
        // Обычное существительное после числа конвертации не мешает.
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
    }

    #[test]
    fn units_get_a_non_breaking_space_after_a_number() {
        let rule = Whitespace {
            non_breaking_units: true,
        };
        assert_eq!(rule.apply("5 кг картошки", "ru"), "5\u{a0}кг картошки");
        assert_eq!(rule.apply("рост 10 %", "ru"), "рост 10\u{a0}%");
        // Слово, похожее на единицу, но без числа перед ним — обычный пробел.
        assert_eq!(rule.apply("просто кг", "ru"), "просто кг");
    }

    #[test]
    fn capitalisation_starts_sentences_and_lines_only() {
        assert_eq!(
            Capitalize.apply("привет. как дела? хорошо", "ru"),
            "Привет. Как дела? Хорошо"
        );
        assert_eq!(Capitalize.apply("строка\nвторая", "ru"), "Строка\nВторая");
        // Регистр внутри слов не меняется: идентификаторы кода остаются собой.
        assert_eq!(
            Capitalize.apply("вызови getUserById потом exit", "ru"),
            "Вызови getUserById потом exit"
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

    // --- Golden-тесты: реальные фразы целиком -------------------------------------------

    fn golden_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/rules")
            .canonicalize()
            .expect("каталог golden-тестов существует")
    }

    /// Язык берётся из имени файла: `ru-punctuation.in.txt` → `ru`.
    fn lang_of(name: &str) -> String {
        name.split('-').next().unwrap_or("ru").to_string()
    }

    #[test]
    fn golden_cases_match_the_recorded_output() {
        let dir = golden_dir();
        let update = std::env::var("UPDATE_GOLDEN").is_ok();
        let mut cases: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("golden-каталог читается")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.to_string_lossy().ends_with(".in.txt"))
            .collect();
        cases.sort();
        assert!(!cases.is_empty(), "нет ни одного golden-случая в {dir:?}");

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
                .unwrap_or_else(|_| panic!("нет эталона {expected_path:?}; UPDATE_GOLDEN=1"));
            let expected = expected.trim_end_matches('\n');
            if actual != expected {
                failures.push(format!(
                    "{name}:\n  вход:    {input:?}\n  ожидали: {expected:?}\n  вышло:   {actual:?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "golden-случаи разошлись с эталоном:\n{}",
            failures.join("\n")
        );
    }
}
