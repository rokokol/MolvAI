// SPDX-License-Identifier: MIT
//! Распознавание речи: движок whisper.cpp и политика выбора языка.
//!
//! Политика вынесена из движка в чистую функцию: она проверяется на фейке без модели и работает
//! одинаково для любого движка (whisper.cpp, облачный).

pub mod whisper;

pub use whisper::WhisperEngine;

use std::path::{Path, PathBuf};

use tracing::debug;

use crate::config::SttConfig;
use crate::domain::audio::PcmAudio;
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions, Transcript};

/// Параметры распознавания из настроек: одно место, где конфиг превращается в `SttOptions`.
///
/// `initial_prompt` заполняет словарь терминов уже на стороне конвейера — он знает,
/// какие термины актуальны для этой реплики.
pub fn stt_options_from_config(cfg: &SttConfig, timestamps: bool) -> SttOptions {
    SttOptions {
        language: LanguageHint::parse(&cfg.language),
        allowed_languages: cfg.allowed_languages.clone(),
        initial_prompt: None,
        threads: cfg.threads as usize,
        timestamps,
    }
}

/// Путь к файлу весов: явный `model_path` из настроек либо имя модели в каталоге моделей.
///
/// Каталог передаётся параметром, потому что им владеет каталог моделей (`molva models`), а не
/// движок: так путь считается одинаково и в демоне, и в CLI.
pub fn model_file_path(cfg: &SttConfig, models_dir: &Path) -> PathBuf {
    let explicit = cfg.model_path.trim();
    if !explicit.is_empty() {
        return PathBuf::from(explicit);
    }
    models_dir.join(format!("ggml-{}.bin", cfg.model.trim()))
}

/// Фразы, которые whisper выдаёт на тишине и шуме, независимо от того, что записали.
///
/// Модель обучалась на субтитрах, поэтому на пустом входе уверенно печатает концовку ролика.
/// Сравнение идёт по нормализованной форме: без регистра, знаков препинания и лишних пробелов.
const SILENCE_HALLUCINATIONS: [&str; 10] = [
    "продолжение следует",
    "субтитры сделал dimatorzok",
    "субтитры создавал dimatorzok",
    "редактор субтитров а синецкая корректор а егорова",
    "спасибо за просмотр",
    "подписывайтесь на канал",
    "thank you for watching",
    "thanks for watching",
    "subscribe to my channel",
    "you",
];

/// Похоже ли, что распознана тишина, а не речь.
///
/// Две проверки: собственная оценка модели `no_speech_prob` (порог из настроек,
/// `stt.no_speech_threshold`) и список фраз, которые whisper печатает на пустом входе. Пустой
/// текст — тоже тишина. Вызывающий по `true` не вставляет ничего (F-22).
pub fn is_silence_hallucination(transcript: &Transcript, no_speech_threshold: f32) -> bool {
    if transcript.text.trim().is_empty() {
        return true;
    }
    if transcript
        .no_speech_prob
        .is_some_and(|p| p >= no_speech_threshold)
    {
        return true;
    }
    let normalised = normalise_for_match(&transcript.text);
    SILENCE_HALLUCINATIONS
        .iter()
        .any(|phrase| normalised == *phrase)
}

/// Нормализация для сравнения с образцами: нижний регистр, только буквы и цифры, один пробел.
fn normalise_for_match(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Распознать с одним повтором, если автоопределение выдало язык вне списка разрешённых.
///
/// Типичная ошибка whisper на коротких репликах — принять русскую речь за украинскую или
/// болгарскую. `allowed_languages` — это языки, которые пользователь вообще использует; если
/// определённый язык не из списка, делаем ровно один повтор с первым языком списка (I-06).
/// При фиксированном языке (I-07) автоопределение выключено и повтор не нужен.
pub fn transcribe_with_language_policy(
    engine: &mut dyn SttEngine,
    audio: &PcmAudio,
    options: &SttOptions,
) -> Result<Transcript, SttError> {
    let first = engine.transcribe(audio, options)?;
    let Some(fallback) = retry_language(options, first.detected_language.as_deref()) else {
        return Ok(first);
    };
    debug!(
        detected = first.detected_language.as_deref().unwrap_or("?"),
        fallback = %fallback,
        "определённый язык вне списка разрешённых, повтор с фиксированным языком"
    );
    let retry_opts = SttOptions {
        language: LanguageHint::Fixed(fallback),
        ..options.clone()
    };
    engine.transcribe(audio, &retry_opts)
}

/// Язык для повторного прогона или `None`, если повтор не нужен.
///
/// Повтор нужен только когда язык выбирала модель, список разрешённых непуст и выбранный язык в
/// него не попал.
pub fn retry_language(options: &SttOptions, detected: Option<&str>) -> Option<String> {
    if options.language != LanguageHint::Auto {
        return None;
    }
    let detected = detected?;
    let first_allowed = options.allowed_languages.first()?;
    let allowed = options
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
        let options = opts_auto(&["ru", "en"]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &options).expect("успех");

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
        let options = SttOptions {
            language: LanguageHint::Fixed("en".into()),
            ..opts_auto(&["ru"])
        };

        let out = transcribe_with_language_policy(&mut engine, &audio(), &options).expect("успех");

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
    fn confident_silence_is_not_inserted() {
        let transcript = Transcript {
            text: "Привет".into(),
            no_speech_prob: Some(0.9),
            ..Transcript::default()
        };
        assert!(is_silence_hallucination(&transcript, 0.6));
    }

    #[test]
    fn real_speech_passes_the_gate() {
        let transcript = Transcript {
            text: "Привет, как дела".into(),
            no_speech_prob: Some(0.05),
            ..Transcript::default()
        };
        assert!(!is_silence_hallucination(&transcript, 0.6));
    }

    #[test]
    fn subtitle_boilerplate_is_recognised_as_hallucination() {
        // Уверенность модели высокая, а текст — концовка ролика из обучающей выборки.
        for text in ["Продолжение следует...", "Субтитры сделал DimaTorzok"]
        {
            let transcript = Transcript {
                text: text.into(),
                no_speech_prob: Some(0.01),
                ..Transcript::default()
            };
            assert!(
                is_silence_hallucination(&transcript, 0.6),
                "не отсеяно: {text}"
            );
        }
    }

    #[test]
    fn empty_text_is_silence() {
        assert!(is_silence_hallucination(&Transcript::default(), 0.6));
        assert!(is_silence_hallucination(&Transcript::text_only("   "), 0.6));
    }

    #[test]
    fn phrase_that_merely_contains_boilerplate_is_kept() {
        // «Спасибо за просмотр отчёта» — это речь пользователя, а не хвост субтитров.
        let transcript = Transcript {
            text: "Спасибо за просмотр отчёта, до связи".into(),
            no_speech_prob: Some(0.02),
            ..Transcript::default()
        };
        assert!(!is_silence_hallucination(&transcript, 0.6));
    }

    #[test]
    fn missing_no_speech_prob_does_not_block_speech() {
        let transcript = Transcript::text_only("привет");
        assert!(!is_silence_hallucination(&transcript, 0.6));
    }

    #[test]
    fn options_come_from_the_config() {
        let cfg = SttConfig {
            language: "RU".into(),
            allowed_languages: vec!["ru".into(), "en".into()],
            threads: 6,
            ..SttConfig::default()
        };

        let options = stt_options_from_config(&cfg, true);

        assert_eq!(options.language, LanguageHint::Fixed("ru".into()));
        assert_eq!(options.allowed_languages, vec!["ru", "en"]);
        assert_eq!(options.threads, 6);
        assert!(options.timestamps);
    }

    #[test]
    fn empty_model_path_falls_back_to_the_models_directory() {
        let cfg = SttConfig {
            model: "small".into(),
            model_path: String::new(),
            ..SttConfig::default()
        };

        assert_eq!(
            model_file_path(&cfg, Path::new("/data/models")),
            PathBuf::from("/data/models/ggml-small.bin")
        );
    }

    #[test]
    fn explicit_model_path_wins_over_the_catalogue() {
        let cfg = SttConfig {
            model: "small".into(),
            model_path: "/opt/whisper/my-model.bin".into(),
            ..SttConfig::default()
        };

        assert_eq!(
            model_file_path(&cfg, Path::new("/data/models")),
            PathBuf::from("/opt/whisper/my-model.bin")
        );
    }

    #[test]
    fn failure_of_the_first_pass_is_returned_as_is() {
        let mut engine =
            FakeStt::with_responses(vec![Err(SttError::Inference("сломалось".into()))]);

        let error = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru"]))
            .expect_err("ошибка движка должна дойти до вызывающего");

        assert_eq!(error, SttError::Inference("сломалось".into()));
    }
}
