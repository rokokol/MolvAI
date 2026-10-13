// SPDX-License-Identifier: MIT
//! Recognition engine on top of whisper.cpp (whisper-rs)
//!
//! The model loads lazily at the first recognition and lives until `unload()`: the context is
//! reused between phrases, so the second phrase does not pay for the load of the weights (R-13/14)

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Instant;

use tracing::{debug, info};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::domain::audio::{PcmAudio, TARGET_SAMPLE_RATE};
use crate::domain::stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};

/// The whisper.cpp logs move from stderr to `tracing` exactly once per process
static LOGGING_HOOKS: Once = Once::new();

/// Recognizer on whisper.cpp: local, no network
#[derive(Debug)]
pub struct WhisperEngine {
    model_path: PathBuf,
    model_name: String,
    /// 0 — derive from the number of logical cores
    threads: usize,
    /// `None` until the model is loaded, and after `unload()`
    context: Option<WhisperContext>,
}

impl WhisperEngine {
    pub fn new(model_path: PathBuf, model_name: String, threads: usize) -> Self {
        Self {
            model_path,
            model_name,
            threads,
            context: None,
        }
    }

    /// Whether the model is loaded in memory right now
    pub fn is_loaded(&self) -> bool {
        self.context.is_some()
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// Load the model if it is not in memory yet
    fn context(&mut self) -> Result<&WhisperContext, SttError> {
        if self.context.is_none() {
            if !self.model_path.exists() {
                return Err(SttError::ModelMissing {
                    path: self.model_path.display().to_string(),
                    model: self.model_name.clone(),
                });
            }
            LOGGING_HOOKS.call_once(whisper_rs::install_logging_hooks);

            let started = Instant::now();
            let ctx = WhisperContext::new_with_params(
                &self.model_path,
                WhisperContextParameters::default(),
            )
            .map_err(|e| SttError::ModelLoad(format!("{}: {e}", self.model_path.display())))?;
            info!(
                model = %self.model_name,
                path = %self.model_path.display(),
                load_ms = started.elapsed().as_millis() as u64,
                "whisper model loaded"
            );
            self.context = Some(ctx);
        }
        // The context was just filled, or it was already filled
        self.context
            .as_ref()
            .ok_or_else(|| SttError::ModelLoad("context not created".into()))
    }
}

impl SttEngine for WhisperEngine {
    fn id(&self) -> &'static str {
        "whisper-cpp"
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn transcribe(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
    ) -> Result<Transcript, SttError> {
        if audio.samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        let language = whisper_language(&options.language)?;
        let threads = resolve_threads(options.threads.max(self.threads));

        // whisper.cpp takes only mono 16 kHz; the conversion here protects the caller
        let resampled;
        let samples = if audio.sample_rate == TARGET_SAMPLE_RATE {
            &audio.samples
        } else {
            resampled = audio.to_16k();
            &resampled.samples
        };

        let mut state = self
            .context()?
            .create_state()
            .map_err(|e| SttError::ModelLoad(format!("whisper state: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads as i32);
        params.set_translate(false);
        params.set_no_timestamps(!options.timestamps);
        // The whisper.cpp progress and text must not reach stdout: it carries only data (Y-15)
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // The temperature fallback stays at the default: with `temperature_inc = 0` whisper.cpp 1.8
        // returns no segments at all (checked on the fixtures), and it saves no time.
        // `detect_language = true` in whisper.cpp means "only detect the language and exit":
        // there will be no segments at all. Auto-detection together with recognition is the
        // language "auto"
        params.set_detect_language(false);
        params.set_language(Some(language.as_deref().unwrap_or("auto")));
        // The whisper encoder always computes a 30-second window, even if the phrase lasted four:
        // 85 % of the work goes to padded silence. The window shrinks to the length of the
        // phrase — this is the main lever for latency on the CPU
        params.set_audio_ctx(audio_ctx_for(samples.len(), TARGET_SAMPLE_RATE) as i32);
        if let Some(prompt) = options.initial_prompt.as_deref() {
            // A zero byte inside the prompt would crash CString in whisper-rs
            if !prompt.is_empty() && !prompt.contains('\0') {
                params.set_initial_prompt(prompt);
            }
        }

        let started = Instant::now();
        state
            .full(params, samples)
            .map_err(|e| SttError::Inference(e.to_string()))?;
        let elapsed = started.elapsed();

        let mut transcript = collect_transcript(&state)?;
        if !options.timestamps {
            transcript.segments.clear();
        }
        debug!(
            model = %self.model_name,
            threads,
            audio_secs = audio.duration_secs(),
            stt_ms = elapsed.as_millis() as u64,
            detected = transcript.detected_language.as_deref().unwrap_or("?"),
            "recognition finished"
        );
        Ok(transcript)
    }

    fn unload(&mut self) {
        if self.context.take().is_some() {
            info!(model = %self.model_name, "whisper model unloaded");
        }
    }

    /// Detect the language from the first seconds, choosing only among the allowed ones
    ///
    /// It is cheaper than the `auto` mode of the recognition itself by about an order of
    /// magnitude, and this is why. whisper.cpp detects the language inside `whisper_full`
    /// **before** it applies `audio_ctx`, so always on the full thirty-second window: on the CPU
    /// this is the single most expensive encoder run, and it is why `auto` costs twenty seconds.
    /// There is only one way around this: first a short run with a known language — it shrinks the
    /// encoder window in the state — and only then the language detection on the same
    /// spectrogram, now on the shrunk window
    ///
    /// From the probabilities the best language **from the list of the user** is taken: on a
    /// short Russian phrase whisper easily chooses Ukrainian or Bulgarian, and the list has no such
    /// languages
    fn detect_language(&mut self, audio: &PcmAudio, opts: &SttOptions) -> Option<String> {
        let allowed = allowed_language_ids(&opts.allowed_languages);
        match allowed.len() {
            0 => return None,
            // Exactly one language is allowed — there is nothing to choose and nothing to compute
            1 => return Some(allowed[0].1.clone()),
            _ => {}
        }
        let resampled;
        let samples = if audio.sample_rate == TARGET_SAMPLE_RATE {
            &audio.samples
        } else {
            resampled = audio.to_16k();
            &resampled.samples
        };
        let samples = first_seconds(samples, TARGET_SAMPLE_RATE, DETECT_SECS);
        if samples.is_empty() {
            return None;
        }

        let threads = resolve_threads(opts.threads.max(self.threads));
        let audio_ctx = detect_audio_ctx(samples.len(), TARGET_SAMPLE_RATE);
        let started = Instant::now();
        let mut state = self.context().ok()?.create_state().ok()?;

        // The first run is needed for its side effect, not for the text: `whisper_full` writes
        // `audio_ctx` into the state, and the next encoder run computes the short window instead of the full one
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads as i32);
        params.set_translate(false);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_detect_language(false);
        params.set_language(Some(allowed[0].1.as_str()));
        params.set_audio_ctx(audio_ctx as i32);
        // The text of this run is not needed, so the decoding is cut to the limit: one segment,
        // one token and no temperature fallback. Otherwise, on speech in the "wrong" language,
        // whisper goes into retries and the run becomes four times longer
        params.set_single_segment(true);
        params.set_max_tokens(1);
        params.set_temperature_inc(0.0);
        state.full(params, samples).ok()?;

        let (_best, probs) = state.lang_detect(0, threads).ok()?;
        let chosen = best_allowed_language(&probs, &allowed);
        debug!(
            detect_ms = started.elapsed().as_millis() as u64,
            audio_ctx,
            chosen = chosen.as_deref().unwrap_or("?"),
            "language detected from the first seconds"
        );
        chosen
    }
}

/// How many seconds at the start of a phrase are enough to find the language
const DETECT_SECS: f32 = 2.0;

/// Encoder window for the language detection: exactly the size of the fragment, with no margin
///
/// The margin [`AUDIO_CTX_MARGIN`] protects the tail of the last word, but the language does not
/// need the tail — it needs only recognizable phonetics. Each extra window position costs time
/// twice here: in the run that shrinks the window and in the detection itself
fn detect_audio_ctx(samples: usize, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return FULL_AUDIO_CTX;
    }
    let secs = samples.div_ceil(sample_rate as usize);
    (secs * AUDIO_CTX_PER_SEC).clamp(AUDIO_CTX_PER_SEC, FULL_AUDIO_CTX)
}

/// The first `secs` seconds of the signal
fn first_seconds(samples: &[f32], sample_rate: u32, secs: f32) -> &[f32] {
    let limit = (sample_rate as f32 * secs.max(0.0)) as usize;
    &samples[..samples.len().min(limit.max(1))]
}

/// The allowed languages as pairs of "number in the whisper table, code"
///
/// Unknown codes are dropped silently: the list comes from the settings of the user, and a typo
/// in it is no reason to refuse the detection among the other languages
fn allowed_language_ids(allowed: &[String]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for code in allowed {
        let code = code.trim().to_lowercase();
        let Some(id) = whisper_rs::get_lang_id(&code) else {
            continue;
        };
        let Ok(id) = usize::try_from(id) else {
            continue;
        };
        if !out.iter().any(|(known, _)| *known == id) {
            out.push((id, code));
        }
    }
    out
}

/// The most probable language among the allowed ones
fn best_allowed_language(probs: &[f32], allowed: &[(usize, String)]) -> Option<String> {
    allowed
        .iter()
        .filter_map(|(id, code)| probs.get(*id).map(|p| (*p, code)))
        .filter(|(p, _)| p.is_finite())
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, code)| code.clone())
}

/// Collect the text, the segments, the language and the silence probability from the whisper state
fn collect_transcript(state: &WhisperState) -> Result<Transcript, SttError> {
    let mut text = String::new();
    let mut segments = Vec::new();
    let mut no_speech = Vec::new();

    for segment in state.as_iter() {
        let piece = segment
            .to_str_lossy()
            .map_err(|e| SttError::Inference(format!("segment text: {e}")))?;
        text.push_str(&piece);
        no_speech.push(segment.no_speech_probability());
        segments.push(Segment {
            // whisper returns timestamps in hundredths of a second
            start_ms: centiseconds_to_ms(segment.start_timestamp()),
            end_ms: centiseconds_to_ms(segment.end_timestamp()),
            text: piece.trim().to_string(),
        });
    }

    let detected_language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
        .filter(|lang| *lang != "auto")
        .map(str::to_string);

    Ok(Transcript {
        text: text.trim().to_string(),
        segments,
        detected_language,
        no_speech_prob: aggregate_no_speech(&no_speech),
    })
}

fn centiseconds_to_ms(value: i64) -> u32 {
    value
        .max(0)
        .saturating_mul(10)
        .try_into()
        .unwrap_or(u32::MAX)
}

/// Full encoder window of whisper: 30 seconds of audio
const FULL_AUDIO_CTX: usize = 1500;

/// Encoder positions per second of audio
const AUDIO_CTX_PER_SEC: usize = FULL_AUDIO_CTX / 30;

/// Margin on top of the phrase length: the tail of a word must not fall on the edge of the window
const AUDIO_CTX_MARGIN: usize = 128;

/// Encoder window size for a given recording length
///
/// whisper pads the input to 30 seconds and honestly computes the convolutions over all the
/// silence. For a phrase of four seconds this is five times more work than needed. We shrink the
/// window to the length of the phrase plus a margin, but not below half a second: on very short
/// windows the model starts to make errors
fn audio_ctx_for(samples: usize, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return FULL_AUDIO_CTX;
    }
    let secs = samples.div_ceil(sample_rate as usize);
    let needed = secs * AUDIO_CTX_PER_SEC + AUDIO_CTX_MARGIN;
    needed.clamp(AUDIO_CTX_MARGIN, FULL_AUDIO_CTX)
}

/// Final probability of no speech — the minimum over the segments
///
/// If the model is sure in even one segment that there is speech, the recording is not silence:
/// taking the maximum would drop the phrase because of one tail segment with noise
fn aggregate_no_speech(probs: &[f32]) -> Option<f32> {
    probs
        .iter()
        .copied()
        .filter(|p| p.is_finite())
        .fold(None, |acc: Option<f32>, p| {
            Some(acc.map_or(p, |a| a.min(p)))
        })
}

/// Language code for whisper: `None` means auto-detection
///
/// The codes come from the config, so we check the form: whisper-rs panics on a zero byte
fn whisper_language(hint: &LanguageHint) -> Result<Option<String>, SttError> {
    match hint {
        LanguageHint::Auto => Ok(None),
        LanguageHint::Fixed(code) => {
            let code = code.trim().to_lowercase();
            let valid = !code.is_empty()
                && code.len() <= 8
                && code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !valid {
                return Err(SttError::Unsupported(format!("language code `{code}`")));
            }
            Ok(Some(code))
        }
    }
}

/// 0 means half of the logical cores, but not less than one: whisper.cpp on all cores takes the
/// CPU from the rest of the system, and the gain after half is almost zero
fn resolve_threads(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    (cores / 2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_file_names_the_model_and_path() {
        let mut engine = WhisperEngine::new(
            PathBuf::from("/nonexistent/ggml-small.bin"),
            "small".into(),
            1,
        );
        let audio = PcmAudio::new(vec![0.1; 16_000], 16_000);

        let error = engine
            .transcribe(&audio, &SttOptions::default())
            .expect_err("no model");

        assert_eq!(
            error,
            SttError::ModelMissing {
                path: "/nonexistent/ggml-small.bin".into(),
                model: "small".into()
            }
        );
        assert!(!engine.is_loaded());
    }

    #[test]
    fn empty_audio_is_rejected_before_loading_the_model() {
        let mut engine = WhisperEngine::new(PathBuf::from("/nonexistent"), "small".into(), 1);
        let error = engine
            .transcribe(&PcmAudio::default(), &SttOptions::default())
            .expect_err("empty audio");
        assert_eq!(error, SttError::EmptyAudio);
    }

    #[test]
    fn unload_is_safe_without_loaded_model() {
        let mut engine = WhisperEngine::new(PathBuf::from("/nonexistent"), "small".into(), 1);
        engine.unload();
        assert!(!engine.is_loaded());
    }

    #[test]
    fn engine_reports_id_and_model() {
        let engine = WhisperEngine::new(PathBuf::from("/tmp/ggml-tiny.bin"), "tiny".into(), 4);
        assert_eq!(engine.id(), "whisper-cpp");
        assert_eq!(engine.model_name(), "tiny");
    }

    #[test]
    fn auto_hint_means_no_language_for_whisper() {
        assert_eq!(whisper_language(&LanguageHint::Auto).expect("auto"), None);
    }

    #[test]
    fn fixed_language_is_normalised() {
        assert_eq!(
            whisper_language(&LanguageHint::Fixed(" RU ".into())).expect("ru"),
            Some("ru".into())
        );
    }

    #[test]
    fn broken_language_code_is_an_error_not_a_panic() {
        // A zero byte and garbage from the config must not reach CString in whisper-rs
        assert!(whisper_language(&LanguageHint::Fixed("ru\0".into())).is_err());
        assert!(whisper_language(&LanguageHint::Fixed(String::new())).is_err());
        assert!(whisper_language(&LanguageHint::Fixed("русский".into())).is_err());
    }

    #[test]
    fn explicit_thread_count_is_respected() {
        assert_eq!(resolve_threads(6), 6);
    }

    #[test]
    fn zero_threads_resolve_to_at_least_one() {
        assert!(resolve_threads(0) >= 1);
    }

    #[test]
    fn no_speech_is_the_most_confident_segment() {
        assert_eq!(aggregate_no_speech(&[0.9, 0.2, 0.8]), Some(0.2));
        assert_eq!(aggregate_no_speech(&[]), None);
        assert_eq!(aggregate_no_speech(&[f32::NAN, 0.5]), Some(0.5));
    }

    #[test]
    fn audio_ctx_shrinks_with_the_utterance() {
        // Four seconds of speech: the window is five times smaller than the full one, with a margin
        let four_seconds = audio_ctx_for(4 * 16_000, 16_000);
        assert!(
            four_seconds < FULL_AUDIO_CTX / 3,
            "the window did not shrink: {four_seconds}"
        );
        assert!(
            four_seconds >= 4 * AUDIO_CTX_PER_SEC,
            "the window is shorter than the phrase: {four_seconds}"
        );
    }

    #[test]
    fn audio_ctx_never_exceeds_the_full_window() {
        assert_eq!(audio_ctx_for(60 * 16_000, 16_000), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx_for(1_000, 0), FULL_AUDIO_CTX);
    }

    #[test]
    fn very_short_audio_keeps_a_workable_window() {
        assert!(audio_ctx_for(160, 16_000) >= AUDIO_CTX_MARGIN);
    }

    #[test]
    fn the_best_language_is_chosen_only_among_the_allowed_ones() {
        let allowed = allowed_language_ids(&["ru".into(), "en".into()]);
        assert_eq!(allowed.len(), 2);
        // The probabilities are invented: what matters is that the maximum is taken over the allowed ones, not over all
        let mut probs = vec![0.0; 100];
        for (id, code) in &allowed {
            probs[*id] = if code == "en" { 0.7 } else { 0.2 };
        }
        // Ukrainian is more probable than both, but it is not in the list
        if let Some(uk) = whisper_rs::get_lang_id("uk") {
            probs[uk as usize] = 0.9;
        }

        assert_eq!(
            best_allowed_language(&probs, &allowed).as_deref(),
            Some("en")
        );
    }

    #[test]
    fn an_unknown_language_code_is_skipped_not_fatal() {
        let allowed = allowed_language_ids(&["ru".into(), "elvish".into(), "RU".into()]);
        assert_eq!(
            allowed.len(),
            1,
            "the typo is dropped, the duplicate is merged"
        );
        assert_eq!(allowed[0].1, "ru");
        assert_eq!(best_allowed_language(&[], &allowed), None);
    }

    #[test]
    fn the_detection_window_has_no_margin() {
        // Two seconds are a hundred window positions, fifteen times smaller than the full window:
        // all the saving against the `auto` mode depends on this
        assert_eq!(detect_audio_ctx(2 * 16_000, 16_000), 100);
        assert!(detect_audio_ctx(2 * 16_000, 16_000) < audio_ctx_for(2 * 16_000, 16_000));
        assert_eq!(detect_audio_ctx(100, 0), FULL_AUDIO_CTX);
        assert!(detect_audio_ctx(160, 16_000) >= AUDIO_CTX_PER_SEC);
    }

    #[test]
    fn only_the_first_seconds_are_used_for_detection() {
        let samples = vec![0.1; 16_000 * 10];
        assert_eq!(
            first_seconds(&samples, 16_000, DETECT_SECS).len(),
            16_000 * DETECT_SECS as usize
        );
        // A recording shorter than the window is taken whole
        assert_eq!(
            first_seconds(&samples[..1_000], 16_000, DETECT_SECS).len(),
            1_000
        );
        assert!(first_seconds(&[], 16_000, DETECT_SECS).is_empty());
    }

    #[test]
    fn centiseconds_become_milliseconds() {
        assert_eq!(centiseconds_to_ms(0), 0);
        assert_eq!(centiseconds_to_ms(123), 1230);
        assert_eq!(centiseconds_to_ms(-5), 0);
    }

    /// Run on a real model: turned on by hand, when the path is set in `MOLVA_TEST_MODEL`
    ///
    /// `cargo test -p molva-core -- --ignored real_model` with
    /// `MOLVA_TEST_MODEL=~/.local/share/molva/models/ggml-small.bin`
    #[test]
    #[ignore = "needs a downloaded whisper model: MOLVA_TEST_MODEL=<path> --ignored"]
    fn real_model_transcribes_silence_without_crashing() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("set MOLVA_TEST_MODEL=<path to ggml-*.bin>");
        };
        let mut engine = WhisperEngine::new(PathBuf::from(path), "test".into(), 2);
        let audio = PcmAudio::new(
            vec![0.0; TARGET_SAMPLE_RATE as usize * 2],
            TARGET_SAMPLE_RATE,
        );

        let out = engine
            .transcribe(&audio, &SttOptions::default())
            .expect("silence is recognized with no error");

        assert!(
            engine.is_loaded(),
            "the model stayed in memory for the next phrase"
        );
        assert!(
            out.no_speech_prob.unwrap_or(0.0) > 0.5,
            "on silence no_speech_prob must be high, got {:?}",
            out.no_speech_prob
        );
        engine.unload();
        assert!(!engine.is_loaded());
    }

    /// Regression: with `LanguageHint::Auto` the text disappeared completely
    ///
    /// `detect_language = true` in whisper.cpp means "detect the language and exit": the answer
    /// had the language, but no segments and no text. Auto-detection together with recognition
    /// is turned on by the language `"auto"`, not by this flag
    #[test]
    #[ignore = "needs a downloaded whisper model: MOLVA_TEST_MODEL=<path> --ignored"]
    fn regression_auto_language_still_returns_text() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("set MOLVA_TEST_MODEL=<path to ggml-*.bin>");
        };
        let wav = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/privet_ru.wav"
        );
        let audio = read_wav_fixture(wav);

        let mut engine = WhisperEngine::new(PathBuf::from(path), "test".into(), 0);
        let options = SttOptions {
            language: LanguageHint::Auto,
            timestamps: true,
            ..SttOptions::default()
        };

        let out = engine
            .transcribe(&audio, &options)
            .expect("speech is recognized");

        assert!(
            !out.text.trim().is_empty(),
            "the text disappeared with language auto-detection"
        );
        assert_eq!(out.detected_language.as_deref(), Some("ru"));
        assert!(
            !out.segments.is_empty(),
            "timestamps were requested but did not come"
        );
    }

    /// A phrase with no timestamp request still gives text, and the segments do not go out
    #[test]
    #[ignore = "needs a downloaded whisper model: MOLVA_TEST_MODEL=<path> --ignored"]
    fn fixed_language_without_timestamps_returns_text_only() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("set MOLVA_TEST_MODEL=<path to ggml-*.bin>");
        };
        let wav = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/privet_ru.wav"
        );
        let audio = read_wav_fixture(wav);

        let mut engine = WhisperEngine::new(PathBuf::from(path), "test".into(), 0);
        let options = SttOptions {
            language: LanguageHint::Fixed("ru".into()),
            timestamps: false,
            ..SttOptions::default()
        };

        let out = engine
            .transcribe(&audio, &options)
            .expect("speech is recognized");

        assert!(!out.text.trim().is_empty(), "the text disappeared");
        assert!(
            out.segments.is_empty(),
            "segments were not requested but came"
        );
    }

    /// Measurement on a real model: what the language choice among the allowed ones costs
    ///
    /// It prints the time of `Fixed` and `DetectAmong` on three speech fixtures and checks that
    /// the language is chosen right. The numbers from this run decide the default of `stt.language`
    #[test]
    #[ignore = "needs a downloaded whisper model: MOLVA_TEST_MODEL=<path> --ignored"]
    fn real_model_language_detection_is_cheap_and_right() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("set MOLVA_TEST_MODEL=<path to ggml-*.bin>");
        };
        let fixtures = [
            ("privet_ru.wav", "ru"),
            ("hello_en.wav", "en"),
            ("secret_ru_en.wav", "ru"),
        ];
        let mut engine = WhisperEngine::new(PathBuf::from(path), "test".into(), 0);
        let allowed = vec!["ru".to_string(), "en".to_string()];

        for (name, expected) in fixtures {
            let audio = read_wav_fixture(&format!(
                "{}/../../tests/fixtures/{name}",
                env!("CARGO_MANIFEST_DIR")
            ));

            let fixed_opts = SttOptions {
                language: LanguageHint::Fixed(expected.to_string()),
                allowed_languages: allowed.clone(),
                ..SttOptions::default()
            };
            let started = Instant::now();
            let fixed = engine.transcribe(&audio, &fixed_opts).expect("speech");
            let fixed_ms = started.elapsed().as_millis();

            let auto_opts = SttOptions {
                language: LanguageHint::Auto,
                allowed_languages: allowed.clone(),
                ..SttOptions::default()
            };
            let started = Instant::now();
            let language = engine.detect_language(&audio, &auto_opts);
            let detect_ms = started.elapsed().as_millis();

            // The full policy path: one language detection plus one recognition
            let started = Instant::now();
            let detected =
                crate::infra::stt::transcribe_with_language_policy(&mut engine, &audio, &auto_opts)
                    .expect("speech");
            let policy_ms = started.elapsed().as_millis();

            println!(
                "{name}: fixed {fixed_ms} ms | detect {detect_ms} ms | detect_among \
                 {policy_ms} ms (+{} %) | language {language:?}\n  fixed: {}\n  auto:  {}",
                (policy_ms.saturating_sub(fixed_ms)) * 100 / fixed_ms.max(1),
                fixed.text.trim(),
                detected.text.trim()
            );
            assert_eq!(
                language.as_deref(),
                Some(expected),
                "{name}: the language is chosen wrong"
            );
            assert!(
                !detected.text.trim().is_empty(),
                "{name}: the text disappeared with the language choice"
            );
        }
    }

    /// Small WAV reader for manual runs: the fixtures are recorded as 16-bit mono 16 kHz
    #[cfg(test)]
    fn read_wav_fixture(path: &str) -> PcmAudio {
        let mut reader = hound::WavReader::open(path).expect("the fixture is in place");
        let spec = reader.spec();
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.expect("the sample is readable")) / f32::from(i16::MAX))
            .collect();
        PcmAudio::new(
            crate::domain::audio::downmix_to_mono(&samples, spec.channels),
            spec.sample_rate,
        )
    }
}
