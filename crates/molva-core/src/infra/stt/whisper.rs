// SPDX-License-Identifier: MIT
//! Движок распознавания поверх whisper.cpp (whisper-rs).
//!
//! Модель загружается лениво при первом распознавании и живёт до `unload()`: между репликами
//! контекст переиспользуется, поэтому вторая реплика не платит за загрузку весов (R-13/14).

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Instant;

use tracing::{debug, info};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::domain::audio::{PcmAudio, TARGET_SAMPLE_RATE};
use crate::domain::stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};

/// Логи whisper.cpp уводятся из stderr в `tracing` ровно один раз на процесс.
static LOGGING_HOOKS: Once = Once::new();

/// Распознаватель на whisper.cpp: локальный, без сети.
pub struct WhisperEngine {
    model_path: PathBuf,
    model_name: String,
    /// 0 — определить по числу логических ядер.
    threads: usize,
    /// `None`, пока модель не загружена или после `unload()`.
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

    /// Загружена ли модель в память прямо сейчас.
    pub fn is_loaded(&self) -> bool {
        self.context.is_some()
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// Загрузить модель, если она ещё не в памяти.
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
                "модель whisper загружена"
            );
            self.context = Some(ctx);
        }
        // Только что заполнили или уже было заполнено.
        self.context
            .as_ref()
            .ok_or_else(|| SttError::ModelLoad("контекст не создан".into()))
    }
}

impl SttEngine for WhisperEngine {
    fn id(&self) -> &str {
        "whisper-cpp"
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn transcribe(&mut self, audio: &PcmAudio, opts: &SttOptions) -> Result<Transcript, SttError> {
        if audio.samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        let language = whisper_language(&opts.language)?;
        let threads = resolve_threads(opts.threads.max(self.threads));

        // whisper.cpp принимает только моно 16 кГц; приведение здесь страхует вызывающего.
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
            .map_err(|e| SttError::ModelLoad(format!("состояние whisper: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads as i32);
        params.set_translate(false);
        params.set_no_timestamps(!opts.timestamps);
        // Прогресс и текст whisper.cpp не должны попадать в stdout: там только данные (Y-15).
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        match language.as_deref() {
            Some(code) => {
                params.set_detect_language(false);
                params.set_language(Some(code));
            }
            None => {
                params.set_detect_language(true);
                params.set_language(None);
            }
        }
        if let Some(prompt) = opts.initial_prompt.as_deref() {
            // Нулевой байт внутри подсказки уронил бы CString в whisper-rs.
            if !prompt.is_empty() && !prompt.contains('\0') {
                params.set_initial_prompt(prompt);
            }
        }

        let started = Instant::now();
        state
            .full(params, samples)
            .map_err(|e| SttError::Inference(e.to_string()))?;
        let elapsed = started.elapsed();

        let transcript = collect_transcript(&state)?;
        debug!(
            model = %self.model_name,
            threads,
            audio_secs = audio.duration_secs(),
            stt_ms = elapsed.as_millis() as u64,
            detected = transcript.detected_language.as_deref().unwrap_or("?"),
            "распознавание завершено"
        );
        Ok(transcript)
    }

    fn unload(&mut self) {
        if self.context.take().is_some() {
            info!(model = %self.model_name, "модель whisper выгружена");
        }
    }
}

/// Собрать текст, сегменты, язык и вероятность тишины из состояния whisper.
fn collect_transcript(state: &WhisperState) -> Result<Transcript, SttError> {
    let mut text = String::new();
    let mut segments = Vec::new();
    let mut no_speech = Vec::new();

    for segment in state.as_iter() {
        let piece = segment
            .to_str_lossy()
            .map_err(|e| SttError::Inference(format!("текст сегмента: {e}")))?;
        text.push_str(&piece);
        no_speech.push(segment.no_speech_probability());
        segments.push(Segment {
            // whisper отдаёт таймкоды в сотых долях секунды.
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

/// Итоговая вероятность отсутствия речи — минимум по сегментам.
///
/// Если хоть в одном сегменте модель уверена, что речь есть, запись не тишина: брать максимум
/// значило бы выбрасывать реплику из-за одного хвостового сегмента с шумом.
fn aggregate_no_speech(probs: &[f32]) -> Option<f32> {
    probs
        .iter()
        .copied()
        .filter(|p| p.is_finite())
        .fold(None, |acc: Option<f32>, p| {
            Some(acc.map_or(p, |a| a.min(p)))
        })
}

/// Код языка для whisper: `None` означает автоопределение.
///
/// Коды приходят из конфига, поэтому проверяем форму: whisper-rs паникует на нулевом байте.
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
                return Err(SttError::Unsupported(format!("код языка `{code}`")));
            }
            Ok(Some(code))
        }
    }
}

/// 0 — половина логических ядер, но не меньше одного: whisper.cpp на всех ядрах отбирает
/// процессор у остальной системы, а выигрыш после половины почти нулевой.
fn resolve_threads(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
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

        let err = engine
            .transcribe(&audio, &SttOptions::default())
            .expect_err("модели нет");

        assert_eq!(
            err,
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
        let err = engine
            .transcribe(&PcmAudio::default(), &SttOptions::default())
            .expect_err("пустое аудио");
        assert_eq!(err, SttError::EmptyAudio);
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
        // Нулевой байт и мусор из конфига не должны доходить до CString в whisper-rs.
        assert!(whisper_language(&LanguageHint::Fixed("ru\0".into())).is_err());
        assert!(whisper_language(&LanguageHint::Fixed("".into())).is_err());
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
    fn centiseconds_become_milliseconds() {
        assert_eq!(centiseconds_to_ms(0), 0);
        assert_eq!(centiseconds_to_ms(123), 1230);
        assert_eq!(centiseconds_to_ms(-5), 0);
    }

    /// Прогон на настоящей модели: включается вручную, когда путь задан в `MOLVA_TEST_MODEL`.
    ///
    /// `cargo test -p molva-core -- --ignored real_model` при
    /// `MOLVA_TEST_MODEL=~/.local/share/molva/models/ggml-small.bin`.
    #[test]
    #[ignore = "нужна скачанная модель whisper: MOLVA_TEST_MODEL=<путь> --ignored"]
    fn real_model_transcribes_silence_without_crashing() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("задайте MOLVA_TEST_MODEL=<путь к ggml-*.bin>");
        };
        let mut engine = WhisperEngine::new(PathBuf::from(path), "test".into(), 2);
        let audio = PcmAudio::new(
            vec![0.0; TARGET_SAMPLE_RATE as usize * 2],
            TARGET_SAMPLE_RATE,
        );

        let out = engine
            .transcribe(&audio, &SttOptions::default())
            .expect("тишина распознаётся без ошибки");

        assert!(
            engine.is_loaded(),
            "модель осталась в памяти для следующей реплики"
        );
        assert!(
            out.no_speech_prob.unwrap_or(0.0) > 0.5,
            "на тишине no_speech_prob должен быть высоким, получено {:?}",
            out.no_speech_prob
        );
        engine.unload();
        assert!(!engine.is_loaded());
    }
}
