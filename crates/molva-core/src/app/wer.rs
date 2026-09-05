// SPDX-License-Identifier: MIT
//! WER и CER: доля ошибок распознавания относительно эталона.
//!
//! Метрика считается по нормализованному тексту — регистр, пунктуация и «ё» не должны
//! превращаться в ошибки, иначе жюри увидит разницу там, где человек её не слышит.
//! Формула классическая: расстояние Левенштейна (замены + вставки + удаления), делённое
//! на длину эталона. Значение больше 1.0 возможно — гипотеза может быть длиннее эталона.

/// Настройки нормализации перед сравнением.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Normalization {
    pub lowercase: bool,
    pub strip_punctuation: bool,
    /// «ё» → «е»: в русских расшифровках это одна и та же буква на слух.
    pub fold_yo: bool,
}

impl Default for Normalization {
    fn default() -> Self {
        Self {
            lowercase: true,
            strip_punctuation: true,
            fold_yo: true,
        }
    }
}

/// Привести текст к сравнимому виду: регистр, пунктуация, пробелы.
pub fn normalize(text: &str, options: Normalization) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let ch = if options.fold_yo {
            match ch {
                'ё' => 'е',
                'Ё' => 'Е',
                other => other,
            }
        } else {
            ch
        };
        if ch.is_whitespace() {
            out.push(' ');
        } else if options.strip_punctuation && !ch.is_alphanumeric() {
            // Апостроф внутри слова (don't) не должен разрывать слово на два.
            if ch == '\'' || ch == '\u{2019}' {
                out.push('\'');
            } else {
                out.push(' ');
            }
        } else if options.lowercase {
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Слова нормализованного текста.
pub fn words(text: &str, options: Normalization) -> Vec<String> {
    normalize(text, options)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Расстояние Левенштейна между последовательностями.
pub fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ai) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, bj) in b.iter().enumerate() {
            let cost = usize::from(ai != bj);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn ratio(distance: usize, reference_len: usize, hypothesis_len: usize) -> f32 {
    if reference_len == 0 {
        // Пустой эталон: пустая гипотеза — идеал, любая непустая — полная ошибка.
        return if hypothesis_len == 0 { 0.0 } else { 1.0 };
    }
    distance as f32 / reference_len as f32
}

/// Доля ошибок по словам.
pub fn wer(reference: &str, hypothesis: &str) -> f32 {
    wer_with(reference, hypothesis, Normalization::default())
}

pub fn wer_with(reference: &str, hypothesis: &str, options: Normalization) -> f32 {
    let r = words(reference, options);
    let h = words(hypothesis, options);
    ratio(levenshtein(&r, &h), r.len(), h.len())
}

/// Доля ошибок по символам; пробелы после нормализации сохраняются как разделители.
pub fn cer(reference: &str, hypothesis: &str) -> f32 {
    cer_with(reference, hypothesis, Normalization::default())
}

pub fn cer_with(reference: &str, hypothesis: &str, options: Normalization) -> f32 {
    let r: Vec<char> = normalize(reference, options).chars().collect();
    let h: Vec<char> = normalize(hypothesis, options).chars().collect();
    ratio(levenshtein(&r, &h), r.len(), h.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn identical_text_has_no_errors() {
        assert!(close(wer("привет мир", "привет мир"), 0.0));
        assert!(close(cer("привет мир", "привет мир"), 0.0));
    }

    #[test]
    fn case_and_punctuation_are_not_errors() {
        assert!(close(wer("Привет, как дела?", "привет как дела"), 0.0));
        assert!(close(cer("Привет, как дела?", "привет как дела"), 0.0));
    }

    #[test]
    fn yo_is_folded_to_e_by_default() {
        assert!(close(wer("ещё раз", "еще раз"), 0.0));
        let strict = Normalization {
            fold_yo: false,
            ..Normalization::default()
        };
        assert!(wer_with("ещё раз", "еще раз", strict) > 0.0);
    }

    #[test]
    fn one_wrong_word_out_of_four_is_a_quarter() {
        assert!(close(wer("а б в г", "а б х г"), 0.25));
    }

    #[test]
    fn deletion_and_insertion_count_as_errors() {
        assert!(close(wer("а б в г", "а б г"), 0.25));
        assert!(close(wer("а б в г", "а б в г д"), 0.25));
    }

    #[test]
    fn completely_wrong_hypothesis_is_one() {
        assert!(close(wer("а б", "х у"), 1.0));
    }

    #[test]
    fn longer_hypothesis_can_exceed_one() {
        assert!(wer("а", "х у з") > 1.0);
    }

    #[test]
    fn empty_reference_is_zero_only_for_empty_hypothesis() {
        assert!(close(wer("", ""), 0.0));
        assert!(close(cer("", ""), 0.0));
        assert!(close(wer("", "что-то"), 1.0));
        assert!(close(cer("", "что-то"), 1.0));
    }

    #[test]
    fn empty_hypothesis_loses_everything() {
        assert!(close(wer("а б в", ""), 1.0));
    }

    #[test]
    fn cer_counts_single_letter_slip_as_small_error() {
        // «кот» → «кит»: одна буква из трёх.
        assert!(close(cer("кот", "кит"), 1.0 / 3.0));
        // По словам это ошибка целиком.
        assert!(close(wer("кот", "кит"), 1.0));
    }

    #[test]
    fn apostrophe_keeps_english_contractions_whole() {
        assert_eq!(
            words("What's up, guys!", Normalization::default()),
            vec!["what's", "up", "guys"]
        );
        assert!(close(wer("What's up", "what's up"), 0.0));
    }

    #[test]
    fn levenshtein_matches_known_values() {
        assert_eq!(levenshtein(b"kitten", b"sitting"), 3);
        assert_eq!(levenshtein::<u8>(&[], b"abc"), 3);
        assert_eq!(levenshtein(b"abc", &[]), 3);
    }

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(
            normalize("  Привет,\n\tмир — !  ", Normalization::default()),
            "привет мир"
        );
    }

    #[test]
    fn digits_survive_normalization() {
        assert_eq!(
            normalize("Дом 12, кв. 3", Normalization::default()),
            "дом 12 кв 3"
        );
    }
}
