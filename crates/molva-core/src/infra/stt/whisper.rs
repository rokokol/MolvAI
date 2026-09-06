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
#[derive(Debug)]
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
        params.set_no_timestamps(!options.timestamps);
        // Прогресс и текст whisper.cpp не должны попадать в stdout: там только данные (Y-15).
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // Откат по температуре оставлен по умолчанию: с `temperature_inc = 0` whisper.cpp 1.8
        // перестаёт отдавать сегменты вовсе (проверено на фикстурах), а времени это не экономит.
        // `detect_language = true` в whisper.cpp означает «только определить язык и выйти»:
        // сегментов не будет вовсе. Автоопределение вместе с распознаванием — это язык "auto".
        params.set_detect_language(false);
        params.set_language(Some(language.as_deref().unwrap_or("auto")));
        // Энкодер whisper всегда считает окно в 30 секунд, даже если реплика длилась четыре:
        // 85 % работы уходит на дополненную тишину. Окно ужимается под длину реплики — это
        // главный рычаг задержки на CPU.
        params.set_audio_ctx(audio_ctx_for(samples.len(), TARGET_SAMPLE_RATE) as i32);
        if let Some(prompt) = options.initial_prompt.as_deref() {
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
            "распознавание завершено"
        );
        Ok(transcript)
    }

    fn unload(&mut self) {
        if self.context.take().is_some() {
            info!(model = %self.model_name, "модель whisper выгружена");
        }
    }

    /// Определить язык по первым секундам, выбирая только среди разрешённых.
    ///
    /// Дешевле режима `auto` у самого распознавания примерно на порядок, и вот почему. whisper.cpp
    /// определяет язык внутри `whisper_full` **до** того, как применит `audio_ctx`, то есть всегда
    /// по полному окну в тридцать секунд: на процессоре это единственный самый дорогой прогон
    /// энкодера, из-за него `auto` и стоит два десятка секунд. Обойти это можно только так: сперва
    /// короткий прогон с известным языком — он и ужимает окно энкодера в состоянии, — а уже потом
    /// определение языка по той же спектрограмме, теперь по ужатому окну.
    ///
    /// Из вероятностей берётся лучший язык **из списка пользователя**: whisper на короткой русской
    /// фразе легко выбирает украинский или болгарский, а таких языков в списке нет.
    fn detect_language(&mut self, audio: &PcmAudio, opts: &SttOptions) -> Option<String> {
        let allowed = allowed_language_ids(&opts.allowed_languages);
        match allowed.len() {
            0 => return None,
            // Разрешён ровно один язык — выбирать не из чего, и считать нечего.
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

        // Первый прогон нужен не текстом, а побочным действием: `whisper_full` записывает
        // `audio_ctx` в состояние, и следующий энкодер посчитает короткое окно вместо полного.
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
        // Текст этого прогона не нужен, поэтому декодирование урезается до предела: один сегмент,
        // один токен и никакого отката по температуре. Иначе на речи «не того» языка whisper
        // уходит в повторные попытки и прогон растягивается вчетверо.
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
            "язык определён по первым секундам"
        );
        chosen
    }
}

/// Сколько секунд начала реплики хватает, чтобы узнать язык.
const DETECT_SECS: f32 = 2.0;

/// Окно энкодера для определения языка: ровно под фрагмент, без запаса.
///
/// Запас [`AUDIO_CTX_MARGIN`] бережёт хвост последнего слова, а языку хвост не нужен — нужна только
/// узнаваемая фонетика. Каждая лишняя позиция окна здесь стоит времени дважды: и на прогоне,
/// который ужимает окно, и на самом определении.
fn detect_audio_ctx(samples: usize, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return FULL_AUDIO_CTX;
    }
    let secs = samples.div_ceil(sample_rate as usize);
    (secs * AUDIO_CTX_PER_SEC).clamp(AUDIO_CTX_PER_SEC, FULL_AUDIO_CTX)
}

/// Первые `secs` секунд сигнала.
fn first_seconds(samples: &[f32], sample_rate: u32, secs: f32) -> &[f32] {
    let limit = (sample_rate as f32 * secs.max(0.0)) as usize;
    &samples[..samples.len().min(limit.max(1))]
}

/// Разрешённые языки как пары «номер в таблице whisper — код».
///
/// Неизвестные коды выбрасываются молча: список приходит из настроек пользователя, а опечатка в нём
/// не повод отказываться от определения по остальным.
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

/// Самый вероятный язык из разрешённых.
fn best_allowed_language(probs: &[f32], allowed: &[(usize, String)]) -> Option<String> {
    allowed
        .iter()
        .filter_map(|(id, code)| probs.get(*id).map(|p| (*p, code)))
        .filter(|(p, _)| p.is_finite())
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, code)| code.clone())
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

/// Полное окно энкодера whisper: 30 секунд аудио.
const FULL_AUDIO_CTX: usize = 1500;

/// Позиций энкодера на секунду звука.
const AUDIO_CTX_PER_SEC: usize = FULL_AUDIO_CTX / 30;

/// Запас поверх длины реплики: хвост слова не должен попасть на границу окна.
const AUDIO_CTX_MARGIN: usize = 128;

/// Размер окна энкодера под конкретную длину записи.
///
/// whisper дополняет вход до 30 секунд и честно считает свёртки по всей тишине. Для реплики в
/// четыре секунды это впятеро больше работы, чем нужно. Ужимаем окно до длины реплики с запасом,
/// но не меньше половины секунды: на совсем коротких окнах модель начинает ошибаться.
fn audio_ctx_for(samples: usize, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return FULL_AUDIO_CTX;
    }
    let secs = samples.div_ceil(sample_rate as usize);
    let needed = secs * AUDIO_CTX_PER_SEC + AUDIO_CTX_MARGIN;
    needed.clamp(AUDIO_CTX_MARGIN, FULL_AUDIO_CTX)
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
            .expect_err("модели нет");

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
            .expect_err("пустое аудио");
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
        // Нулевой байт и мусор из конфига не должны доходить до CString в whisper-rs.
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
        // Четыре секунды речи: окно впятеро меньше полного, но с запасом.
        let four_seconds = audio_ctx_for(4 * 16_000, 16_000);
        assert!(
            four_seconds < FULL_AUDIO_CTX / 3,
            "окно не ужалось: {four_seconds}"
        );
        assert!(
            four_seconds >= 4 * AUDIO_CTX_PER_SEC,
            "окно короче самой реплики: {four_seconds}"
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
        // Вероятности выдуманы: важно, что берётся максимум по разрешённым, а не по всем.
        let mut probs = vec![0.0; 100];
        for (id, code) in &allowed {
            probs[*id] = if code == "en" { 0.7 } else { 0.2 };
        }
        // Украинский вероятнее обоих, но его в списке нет.
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
        let allowed = allowed_language_ids(&["ru".into(), "эльфийский".into(), "RU".into()]);
        assert_eq!(
            allowed.len(),
            1,
            "опечатка выбрасывается, дубль схлопывается"
        );
        assert_eq!(allowed[0].1, "ru");
        assert_eq!(best_allowed_language(&[], &allowed), None);
    }

    #[test]
    fn the_detection_window_has_no_margin() {
        // Две секунды — сто позиций окна, в пятнадцать раз меньше полного: на нём и держится вся
        // экономия против режима `auto`.
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
        // Запись короче окна берётся целиком.
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

    /// Регрессия: при `LanguageHint::Auto` текст пропадал целиком.
    ///
    /// `detect_language = true` в whisper.cpp означает «определи язык и выйди»: язык в ответе
    /// был, сегментов и текста — ни одного. Автоопределение вместе с распознаванием включается
    /// языком `"auto"`, а не этим флагом.
    #[test]
    #[ignore = "нужна скачанная модель whisper: MOLVA_TEST_MODEL=<путь> --ignored"]
    fn regression_auto_language_still_returns_text() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("задайте MOLVA_TEST_MODEL=<путь к ggml-*.bin>");
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
            .expect("речь распознаётся");

        assert!(
            !out.text.trim().is_empty(),
            "при автоопределении языка текст пропал"
        );
        assert_eq!(out.detected_language.as_deref(), Some("ru"));
        assert!(
            !out.segments.is_empty(),
            "таймкоды запрашивались, но не пришли"
        );
    }

    /// Реплика без запроса таймкодов всё равно даёт текст, а сегменты наружу не идут.
    #[test]
    #[ignore = "нужна скачанная модель whisper: MOLVA_TEST_MODEL=<путь> --ignored"]
    fn fixed_language_without_timestamps_returns_text_only() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("задайте MOLVA_TEST_MODEL=<путь к ggml-*.bin>");
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
            .expect("речь распознаётся");

        assert!(!out.text.trim().is_empty(), "текст пропал");
        assert!(
            out.segments.is_empty(),
            "сегменты не запрашивались, но пришли"
        );
    }

    /// Замер на настоящей модели: чего стоит выбор языка среди разрешённых.
    ///
    /// Печатает время `Fixed` и `DetectAmong` по трём речевым фикстурам и проверяет, что язык
    /// выбран правильно. Числа из этого прогона решают, каким быть умолчанию `stt.language`.
    #[test]
    #[ignore = "нужна скачанная модель whisper: MOLVA_TEST_MODEL=<путь> --ignored"]
    fn real_model_language_detection_is_cheap_and_right() {
        let Ok(path) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("задайте MOLVA_TEST_MODEL=<путь к ggml-*.bin>");
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
            let fixed = engine.transcribe(&audio, &fixed_opts).expect("речь");
            let fixed_ms = started.elapsed().as_millis();

            let auto_opts = SttOptions {
                language: LanguageHint::Auto,
                allowed_languages: allowed.clone(),
                ..SttOptions::default()
            };
            let started = Instant::now();
            let language = engine.detect_language(&audio, &auto_opts);
            let detect_ms = started.elapsed().as_millis();

            // Полный путь политики: одно определение языка плюс одно распознавание.
            let started = Instant::now();
            let detected =
                crate::infra::stt::transcribe_with_language_policy(&mut engine, &audio, &auto_opts)
                    .expect("речь");
            let policy_ms = started.elapsed().as_millis();

            println!(
                "{name}: fixed {fixed_ms} мс | detect {detect_ms} мс | detect_among \
                 {policy_ms} мс (+{} %) | язык {language:?}\n  fixed: {}\n  auto:  {}",
                (policy_ms.saturating_sub(fixed_ms)) * 100 / fixed_ms.max(1),
                fixed.text.trim(),
                detected.text.trim()
            );
            assert_eq!(
                language.as_deref(),
                Some(expected),
                "{name}: язык выбран неверно"
            );
            assert!(
                !detected.text.trim().is_empty(),
                "{name}: текст при выборе языка пропал"
            );
        }
    }

    /// Мини-чтение WAV для ручных прогонов: фикстуры записаны как 16-битный моно 16 кГц.
    #[cfg(test)]
    fn read_wav_fixture(path: &str) -> PcmAudio {
        let mut reader = hound::WavReader::open(path).expect("фикстура на месте");
        let spec = reader.spec();
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.expect("отсчёт читается")) / f32::from(i16::MAX))
            .collect();
        PcmAudio::new(
            crate::domain::audio::downmix_to_mono(&samples, spec.channels),
            spec.sample_rate,
        )
    }
}
