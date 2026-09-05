// SPDX-License-Identifier: MIT
//! Распознавание речи: движок whisper.cpp и политика выбора языка.
//!
//! Политика вынесена из движка в чистую функцию: она проверяется на фейке без модели и работает
//! одинаково для любого движка (whisper.cpp, облачный).

pub mod whisper;

pub use whisper::WhisperEngine;

use tracing::debug;

use crate::domain::audio::PcmAudio;
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions, Transcript};

/// Распознать с одним повтором, если автоопределение выдало язык вне списка разрешённых.
///
/// Типичная ошибка whisper на коротких репликах — принять русскую речь за украинскую или
/// болгарскую. `allowed_languages` — это языки, которые пользователь вообще использует; если
/// определённый язык не из списка, делаем ровно один повтор с первым языком списка (I-06).
/// При фиксированном языке (I-07) автоопределение выключено и повтор не нужен.
pub fn transcribe_with_language_policy(
    engine: &mut dyn SttEngine,
    audio: &PcmAudio,
    opts: &SttOptions,
) -> Result<Transcript, SttError> {
    let first = engine.transcribe(audio, opts)?;
    let Some(fallback) = retry_language(opts, first.detected_language.as_deref()) else {
        return Ok(first);
    };
    debug!(
        detected = first.detected_language.as_deref().unwrap_or("?"),
        fallback = %fallback,
        "определённый язык вне списка разрешённых, повтор с фиксированным языком"
    );
    let retry_opts = SttOptions {
        language: LanguageHint::Fixed(fallback),
        ..opts.clone()
    };
    engine.transcribe(audio, &retry_opts)
}

/// Язык для повторного прогона или `None`, если повтор не нужен.
///
/// Повтор нужен только когда язык выбирала модель, список разрешённых непуст и выбранный язык в
/// него не попал.
pub fn retry_language(opts: &SttOptions, detected: Option<&str>) -> Option<String> {
    if opts.language != LanguageHint::Auto {
        return None;
    }
    let detected = detected?;
    let first_allowed = opts.allowed_languages.first()?;
    let allowed = opts
        .allowed_languages
        .iter()
        .any(|lang| lang.eq_ignore_ascii_case(detected));
    if allowed {
        None
    } else {
        Some(first_allowed.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::FakeStt;

    fn audio() -> PcmAudio {
        PcmAudio::new(vec![0.1; 16_000], 16_000)
    }

    fn detected(lang: &str, text: &str) -> Transcript {
        Transcript {
            text: text.into(),
            detected_language: Some(lang.into()),
            ..Transcript::default()
        }
    }

    fn opts_auto(allowed: &[&str]) -> SttOptions {
        SttOptions {
            language: LanguageHint::Auto,
            allowed_languages: allowed.iter().map(|s| (*s).to_string()).collect(),
            ..SttOptions::default()
        }
    }

    #[test]
    fn language_outside_allowed_list_triggers_one_retry() {
        let mut engine = FakeStt::with_responses(vec![
            Ok(detected("uk", "почалося")),
            Ok(detected("ru", "началось")),
        ]);
        let opts = opts_auto(&["ru", "en"]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts).expect("успех");

        assert_eq!(out.text, "началось", "взят результат повтора, а не первый");
        assert_eq!(engine.calls.len(), 2, "повтор должен быть ровно один");
        assert_eq!(engine.calls[0].language, LanguageHint::Auto);
        assert_eq!(
            engine.calls[1].language,
            LanguageHint::Fixed("ru".into()),
            "повтор идёт с первым языком из allowed_languages"
        );
    }

    #[test]
    fn allowed_language_is_not_retried() {
        let mut engine = FakeStt::with_responses(vec![
            Ok(detected("ru", "привет")),
            Ok(detected("ru", "второй прогон")),
        ]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru", "en"]))
            .expect("успех");

        assert_eq!(out.text, "привет");
        assert_eq!(engine.calls.len(), 1, "лишний прогон удваивает задержку");
    }

    #[test]
    fn fixed_language_never_retries() {
        let mut engine = FakeStt::with_responses(vec![
            Ok(detected("uk", "почалося")),
            Ok(detected("ru", "началось")),
        ]);
        let opts = SttOptions {
            language: LanguageHint::Fixed("en".into()),
            ..opts_auto(&["ru"])
        };

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts).expect("успех");

        assert_eq!(out.text, "почалося");
        assert_eq!(engine.calls.len(), 1);
    }

    #[test]
    fn empty_allowed_list_lets_any_language_through() {
        let mut engine = FakeStt::with_responses(vec![Ok(detected("de", "guten tag"))]);

        let out =
            transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&[])).expect("успех");

        assert_eq!(out.text, "guten tag");
        assert_eq!(engine.calls.len(), 1);
    }

    #[test]
    fn unknown_detected_language_is_not_retried() {
        // Движок не сообщил язык — гадать не о чем.
        let mut engine = FakeStt::with_responses(vec![Ok(Transcript::text_only("привет"))]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru"]))
            .expect("успех");

        assert_eq!(out.text, "привет");
        assert_eq!(engine.calls.len(), 1);
    }

    #[test]
    fn detected_language_case_does_not_matter() {
        assert_eq!(retry_language(&opts_auto(&["ru", "en"]), Some("RU")), None);
        assert_eq!(
            retry_language(&opts_auto(&["ru", "en"]), Some("uk")),
            Some("ru".into())
        );
    }

    #[test]
    fn failure_of_the_first_pass_is_returned_as_is() {
        let mut engine =
            FakeStt::with_responses(vec![Err(SttError::Inference("сломалось".into()))]);

        let err = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru"]))
            .expect_err("ошибка движка должна дойти до вызывающего");

        assert_eq!(err, SttError::Inference("сломалось".into()));
    }
}
