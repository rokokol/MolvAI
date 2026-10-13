// SPDX-License-Identifier: MIT
//! Speech recognition: the whisper.cpp engine and the language choice policy
//!
//! The policy is moved out of the engine into a pure function: tests check it on a fake with no
//! model, and it works the same for any engine (whisper.cpp, cloud)

pub mod whisper;

pub use whisper::WhisperEngine;

use std::path::{Path, PathBuf};

use tracing::debug;

use crate::config::SttConfig;
use crate::domain::audio::PcmAudio;
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions, Transcript};

/// How the language of a phrase is chosen
///
/// The difference shows on the CPU: `Fixed` is one model run with a known language, `DetectAmong`
/// is a short detection on the first seconds plus the same run. The `auto` mode of whisper.cpp
/// itself is not used here at all: it computes the full thirty-second window and costs many
/// times more
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguagePolicy {
    /// The settings fix the language: auto-detection is off
    Fixed(String),
    /// The language is chosen among the allowed ones. An empty list means "leave the choice to
    /// the model"
    DetectAmong(Vec<String>),
}

impl LanguagePolicy {
    /// Policy from the settings: `language = "auto"` with a list of allowed languages gives
    /// `DetectAmong`
    pub fn from_config(cfg: &SttConfig) -> Self {
        match LanguageHint::parse(&cfg.language) {
            LanguageHint::Fixed(code) => LanguagePolicy::Fixed(code),
            LanguageHint::Auto => LanguagePolicy::DetectAmong(cfg.allowed_languages.clone()),
        }
    }

    /// Language hint for the engine
    pub fn hint(&self) -> LanguageHint {
        match self {
            LanguagePolicy::Fixed(code) => LanguageHint::Fixed(code.clone()),
            LanguagePolicy::DetectAmong(_) => LanguageHint::Auto,
        }
    }

    /// Languages among which the choice is allowed
    pub fn allowed(&self) -> &[String] {
        match self {
            LanguagePolicy::Fixed(_) => &[],
            LanguagePolicy::DetectAmong(allowed) => allowed,
        }
    }
}

/// Recognition options from the settings: the one place where the config becomes `SttOptions`
///
/// The pipeline fills `initial_prompt` with the dictionary terms on its own side — it knows
/// which terms are relevant for this phrase
pub fn stt_options_from_config(cfg: &SttConfig, timestamps: bool) -> SttOptions {
    SttOptions {
        language: LanguagePolicy::from_config(cfg).hint(),
        allowed_languages: cfg.allowed_languages.clone(),
        initial_prompt: None,
        threads: cfg.threads as usize,
        timestamps,
    }
}

/// Path to the weights file: the explicit `model_path` from the settings, or the model name in
/// the models directory
///
/// The directory is a parameter because the model catalogue (`molva models`) owns it, not the
/// engine: so the path is computed the same way in the daemon and in the CLI
pub fn model_file_path(cfg: &SttConfig, models_dir: &Path) -> PathBuf {
    let explicit = cfg.model_path.trim();
    if !explicit.is_empty() {
        return PathBuf::from(explicit);
    }
    models_dir.join(format!("ggml-{}.bin", cfg.model.trim()))
}

/// Phrases that whisper returns on silence and noise, whatever was recorded
///
/// The model trained on subtitles, so on an empty input it confidently prints the end of a video.
/// The comparison uses the normalised form: no case, no punctuation and no extra spaces
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

/// Whether the recognized result looks like silence, not speech
///
/// There are two checks: the own estimate of the model, `no_speech_prob` (the threshold comes from
/// the settings, `stt.no_speech_threshold`), and the list of phrases that whisper prints on an
/// empty input. Empty text is silence too. On `true` the caller pastes nothing (F-22)
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

/// Normalisation for the comparison with the patterns: lower case, only letters and digits, one
/// space
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

/// Recognize by the language choice policy
///
/// First a cheap detection of the language on the first seconds, among the allowed ones: an engine
/// that can do this avoids the `auto` mode of the recognition itself — and that mode computes the
/// full thirty-second window and costs many times more on the CPU
///
/// If the engine cannot detect, the old path stays: recognize and, if the model chose a language
/// outside the allowed list, repeat exactly once with the first language of the list (I-06). A
/// typical whisper error on short phrases is to take Russian speech for Ukrainian or Bulgarian.
/// With a fixed language (I-07) there is no auto-detection at all and no retry is needed
pub fn transcribe_with_language_policy(
    engine: &mut dyn SttEngine,
    audio: &PcmAudio,
    options: &SttOptions,
) -> Result<Transcript, SttError> {
    if options.language == LanguageHint::Auto && !options.allowed_languages.is_empty() {
        if let Some(code) = engine.detect_language(audio, options) {
            debug!(language = %code, "language detected before recognition");
            let detected = SttOptions {
                language: LanguageHint::Fixed(code),
                ..options.clone()
            };
            return engine.transcribe(audio, &detected);
        }
    }
    let first = engine.transcribe(audio, options)?;
    let Some(fallback) = retry_language(options, first.detected_language.as_deref()) else {
        return Ok(first);
    };
    debug!(
        detected = first.detected_language.as_deref().unwrap_or("?"),
        fallback = %fallback,
        "detected language is outside the allowed list, retry with a fixed language"
    );
    let retry_opts = SttOptions {
        language: LanguageHint::Fixed(fallback),
        ..options.clone()
    };
    engine.transcribe(audio, &retry_opts)
}

/// Language for the repeat run, or `None` if no retry is needed
///
/// A retry is needed only when the model chose the language, the allowed list is not empty and
/// the chosen language is not in it
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

    /// Engine that can detect the language before recognition
    #[derive(Debug)]
    struct DetectingStt {
        inner: FakeStt,
        detected: Option<String>,
        detect_calls: usize,
    }

    impl DetectingStt {
        fn new(detected: Option<&str>, responses: Vec<Result<Transcript, SttError>>) -> Self {
            Self {
                inner: FakeStt::with_responses(responses),
                detected: detected.map(str::to_string),
                detect_calls: 0,
            }
        }
    }

    impl SttEngine for DetectingStt {
        fn id(&self) -> &'static str {
            "detecting"
        }
        fn model_name(&self) -> &'static str {
            "detecting"
        }
        fn transcribe(
            &mut self,
            audio: &PcmAudio,
            opts: &SttOptions,
        ) -> Result<Transcript, SttError> {
            self.inner.transcribe(audio, opts)
        }
        fn unload(&mut self) {
            self.inner.unload();
        }
        fn detect_language(&mut self, _audio: &PcmAudio, _opts: &SttOptions) -> Option<String> {
            self.detect_calls += 1;
            self.detected.clone()
        }
    }

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
    fn a_detected_language_is_recognised_in_one_pass_without_auto() {
        let mut engine = DetectingStt::new(
            Some("en"),
            vec![
                Ok(detected("en", "hello there")),
                Ok(detected("ru", "лишний")),
            ],
        );

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru", "en"]))
            .expect("success");

        assert_eq!(out.text, "hello there");
        assert_eq!(engine.detect_calls, 1);
        assert_eq!(
            engine.inner.calls.len(),
            1,
            "language detection must not add a second model run"
        );
        assert_eq!(
            engine.inner.calls[0].language,
            LanguageHint::Fixed("en".into()),
            "recognition runs with the detected language, not in auto mode"
        );
    }

    #[test]
    fn a_fixed_language_is_never_detected() {
        let mut engine = DetectingStt::new(Some("en"), vec![Ok(detected("ru", "привет"))]);
        let opts = SttOptions {
            language: LanguageHint::Fixed("ru".into()),
            ..opts_auto(&["ru", "en"])
        };

        transcribe_with_language_policy(&mut engine, &audio(), &opts).expect("success");

        assert_eq!(engine.detect_calls, 0, "extra work on every phrase");
        assert_eq!(
            engine.inner.calls[0].language,
            LanguageHint::Fixed("ru".into())
        );
    }

    #[test]
    fn an_engine_that_cannot_detect_falls_back_to_the_retry() {
        let mut engine = DetectingStt::new(
            None,
            vec![
                Ok(detected("uk", "почалося")),
                Ok(detected("ru", "началось")),
            ],
        );

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru", "en"]))
            .expect("success");

        assert_eq!(out.text, "началось");
        assert_eq!(engine.detect_calls, 1);
        assert_eq!(engine.inner.calls.len(), 2);
    }

    #[test]
    fn the_policy_comes_from_the_configuration() {
        let auto = SttConfig {
            language: "auto".into(),
            allowed_languages: vec!["ru".into(), "en".into()],
            ..SttConfig::default()
        };
        assert_eq!(
            LanguagePolicy::from_config(&auto),
            LanguagePolicy::DetectAmong(vec!["ru".into(), "en".into()])
        );
        assert_eq!(
            LanguagePolicy::from_config(&auto).hint(),
            LanguageHint::Auto
        );

        let fixed = SttConfig {
            language: " RU ".into(),
            ..SttConfig::default()
        };
        assert_eq!(
            LanguagePolicy::from_config(&fixed),
            LanguagePolicy::Fixed("ru".into())
        );
        assert!(
            LanguagePolicy::from_config(&fixed).allowed().is_empty(),
            "with a fixed language there is nothing to choose from"
        );
    }

    #[test]
    fn language_outside_allowed_list_triggers_one_retry() {
        let mut engine = FakeStt::with_responses(vec![
            Ok(detected("uk", "почалося")),
            Ok(detected("ru", "началось")),
        ]);
        let options = opts_auto(&["ru", "en"]);

        let out =
            transcribe_with_language_policy(&mut engine, &audio(), &options).expect("success");

        assert_eq!(
            out.text, "началось",
            "the result of the retry is taken, not the first one"
        );
        assert_eq!(engine.calls.len(), 2, "there must be exactly one retry");
        assert_eq!(engine.calls[0].language, LanguageHint::Auto);
        assert_eq!(
            engine.calls[1].language,
            LanguageHint::Fixed("ru".into()),
            "the retry uses the first language from allowed_languages"
        );
    }

    #[test]
    fn allowed_language_is_not_retried() {
        let mut engine = FakeStt::with_responses(vec![
            Ok(detected("ru", "привет")),
            Ok(detected("ru", "второй прогон")),
        ]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru", "en"]))
            .expect("success");

        assert_eq!(out.text, "привет");
        assert_eq!(engine.calls.len(), 1, "an extra run doubles the delay");
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

        let out =
            transcribe_with_language_policy(&mut engine, &audio(), &options).expect("success");

        assert_eq!(out.text, "почалося");
        assert_eq!(engine.calls.len(), 1);
    }

    #[test]
    fn empty_allowed_list_lets_any_language_through() {
        let mut engine = FakeStt::with_responses(vec![Ok(detected("de", "guten tag"))]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&[]))
            .expect("success");

        assert_eq!(out.text, "guten tag");
        assert_eq!(engine.calls.len(), 1);
    }

    #[test]
    fn unknown_detected_language_is_not_retried() {
        // The engine did not report a language — there is nothing to guess
        let mut engine = FakeStt::with_responses(vec![Ok(Transcript::text_only("привет"))]);

        let out = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru"]))
            .expect("success");

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
        // The model is confident, but the text is the end of a video from the training set
        for text in ["Продолжение следует...", "Субтитры сделал DimaTorzok"]
        {
            let transcript = Transcript {
                text: text.into(),
                no_speech_prob: Some(0.01),
                ..Transcript::default()
            };
            assert!(
                is_silence_hallucination(&transcript, 0.6),
                "not filtered out: {text}"
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
        // "Спасибо за просмотр отчёта" is the speech of the user, not a subtitle tail
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
        let mut engine = FakeStt::with_responses(vec![Err(SttError::Inference("broke".into()))]);

        let error = transcribe_with_language_policy(&mut engine, &audio(), &opts_auto(&["ru"]))
            .expect_err("the engine error must reach the caller");

        assert_eq!(error, SttError::Inference("broke".into()));
    }
}
