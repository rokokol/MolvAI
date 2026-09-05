// SPDX-License-Identifier: MIT
//! Потоковая обработка реплики: куски уходят в модель, пока человек ещё говорит.
//!
//! Раньше вся работа начиналась по отпусканию клавиши: реплика в четыре секунды заставляла ждать
//! ещё четыре. Здесь запись режется [`Segmenter`]-ом прямо во время речи, каждый готовый кусок
//! сразу уходит в распознавание, а после отпускания остаётся только хвост — обычно меньше секунды.
//! Ожидание, которое видит человек, сокращается до последнего куска плюс постобработка.
//!
//! Куски идут одной очередью в один рабочий поток, поэтому порядок текстов — это порядок кусков, и
//! перемешаться они не могут по построению. Контекст между кусками передаётся подсказкой:
//! `initial_prompt` следующего куска — хвост уже распознанного текста вместе с подсказкой словаря,
//! так whisper держит стиль, термины и склонения.
//!
//! Язык выбирается по первому куску реплики и применяется ко всем остальным. Смешанную речь внутри
//! одной реплики whisper всё равно не разделяет: он выбирает один язык на весь фрагмент, поэтому
//! менять язык от куска к куску значило бы получить два разных прочтения одной фразы.

use std::time::Instant;

use crate::config::{AudioConfig, SttConfig};
use crate::domain::audio::{AudioSource, PcmAudio};
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions, Transcript};
use crate::infra::stt::{is_silence_hallucination, transcribe_with_language_policy};

use crate::app::audio::segmenter::{Chunk, Segmenter, SegmenterConfig};

/// Сколько символов уже распознанного текста уходит подсказкой в следующий кусок.
///
/// Двести символов — это примерно последнее предложение: модели хватает, чтобы подхватить стиль и
/// термины, и не настолько много, чтобы подсказка начала подменять собой сам звук.
pub const PROMPT_CONTEXT_CHARS: usize = 200;

/// Как часто демон снимает свежий звук с микрофона во время записи.
pub const TICK_MS: u64 = 250;

/// Распознанный кусок реплики.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChunkText {
    /// Пустая строка означает, что в куске не нашлось речи.
    pub text: String,
    /// Язык, на котором кусок прочитан.
    pub language: Option<String>,
    pub stt_ms: u32,
}

/// Что известно о предыдущих кусках, когда конвейер берётся за следующий.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkContext {
    /// Хвост уже распознанного текста — уходит в `initial_prompt`.
    pub previous_text: String,
    /// Язык, выбранный по первому куску реплики.
    pub language: Option<String>,
    /// Номер куска, начиная с нуля.
    pub index: usize,
}

/// Всё, что накопили куски, — начало реплики для конвейера.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkPrefix {
    pub text: String,
    pub language: Option<String>,
    /// Суммарное время распознавания кусков.
    pub stt_ms: u32,
    /// От начала записи до первого распознанного куска.
    pub first_hypothesis_ms: Option<u32>,
    /// Длительность всей реплики: хвост о ней уже не знает.
    pub audio_secs: Option<f32>,
}

impl ChunkPrefix {
    /// Пустой префикс означает «кусков не было»: реплика распознаётся целиком, как раньше.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Копилка распознанных кусков одной реплики.
///
/// Живёт в рабочем потоке демона: он один разбирает очередь кусков, поэтому порядок сохраняется без
/// каких-либо номеров и сортировок.
#[derive(Debug, Default)]
pub struct ChunkAccumulator {
    texts: Vec<String>,
    language: Option<String>,
    stt_ms: u32,
    first_hypothesis_ms: Option<u32>,
}

impl ChunkAccumulator {
    /// Не распозналось ещё ни одного куска с текстом.
    pub fn is_empty(&self) -> bool {
        self.texts.iter().all(|text| text.trim().is_empty())
    }

    /// Контекст для следующего куска.
    pub fn context(&self) -> ChunkContext {
        ChunkContext {
            previous_text: tail_context(&self.draft()),
            language: self.language.clone(),
            index: self.texts.len(),
        }
    }

    /// Принять распознанный кусок; `since_start_ms` — сколько прошло от начала записи.
    pub fn push(&mut self, chunk: ChunkText, since_start_ms: u32) {
        self.stt_ms = self.stt_ms.saturating_add(chunk.stt_ms);
        if self.language.is_none() {
            // Язык реплики выбирается по первому куску, в котором нашлась речь.
            if !chunk.text.trim().is_empty() {
                self.language = chunk.language.clone();
            }
        }
        if self.first_hypothesis_ms.is_none() && !chunk.text.trim().is_empty() {
            self.first_hypothesis_ms = Some(since_start_ms);
        }
        self.texts.push(chunk.text);
    }

    /// Черновик реплики целиком — то, что видит человек во время речи.
    pub fn draft(&self) -> String {
        join_texts(&self.texts)
    }

    /// Забрать накопленное и очиститься под следующую реплику.
    pub fn take_prefix(&mut self, audio_secs: f32) -> ChunkPrefix {
        let prefix = ChunkPrefix {
            text: self.draft(),
            language: self.language.clone(),
            stt_ms: self.stt_ms,
            first_hypothesis_ms: self.first_hypothesis_ms,
            audio_secs: Some(audio_secs),
        };
        self.reset();
        prefix
    }

    /// Забыть накопленное: запись отменили.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Нарезчик на стороне управляющего потока: снимает свежий звук и отдаёт готовые куски.
pub struct ChunkFeeder {
    enabled: bool,
    pause_ms: u32,
    silence_threshold_db: f32,
    segmenter: Option<Segmenter>,
    /// Сколько отсчётов уже ушло в сегментатор.
    fed: usize,
    started: Instant,
}

impl ChunkFeeder {
    pub fn new(stt: &SttConfig, audio: &AudioConfig) -> Self {
        Self {
            enabled: stt.chunked,
            pause_ms: stt.chunk_pause_ms,
            silence_threshold_db: audio.silence_threshold_db,
            segmenter: None,
            fed: 0,
            started: Instant::now(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Начало новой записи: копилка обнуляется.
    pub fn start(&mut self, at: Instant) {
        self.segmenter = None;
        self.fed = 0;
        self.started = at;
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    /// Сколько кусков отдано с начала записи.
    pub fn emitted(&self) -> usize {
        self.segmenter.as_ref().map_or(0, Segmenter::emitted)
    }

    /// Снять свежий звук и забрать то, что дозрело до куска.
    pub fn tick(&mut self, source: &mut dyn AudioSource) -> Vec<Chunk> {
        if !self.enabled {
            return Vec::new();
        }
        let Some(fresh) = source.drain_new_samples() else {
            // Источник не умеет отдавать звук во время записи: реплика пойдёт целиком.
            return Vec::new();
        };
        self.fed += fresh.samples.len();
        let pause = self.pause_ms;
        let threshold = self.silence_threshold_db;
        let segmenter = self.segmenter.get_or_insert_with(|| {
            Segmenter::new(SegmenterConfig::new(fresh.sample_rate, pause, threshold))
        });
        segmenter.push(&fresh.samples)
    }

    /// Хвост записи: последний кусок и всё, что не успели снять тиком.
    ///
    /// Последний элемент — хвост, остальные обычные куски: если между последним тиком и отпусканием
    /// клавиши человек успел договорить целую фразу, в остатке может оказаться сразу несколько.
    pub fn finish(&mut self, full: &PcmAudio) -> Vec<Chunk> {
        let Some(segmenter) = self.segmenter.as_mut() else {
            return Vec::new();
        };
        let mut chunks = Vec::new();
        if full.samples.len() > self.fed {
            chunks.extend(segmenter.push(&full.samples[self.fed..]));
            self.fed = full.samples.len();
        }
        chunks.extend(segmenter.finish());
        chunks
    }

    /// Запись отменили: сегментатор больше не нужен.
    pub fn cancel(&mut self) {
        self.segmenter = None;
        self.fed = 0;
    }
}

/// Распознать один кусок реплики.
///
/// Язык фиксируется по первому куску: whisper всё равно читает фрагмент одним языком, а прыгающий
/// от куска к куску язык дал бы одну фразу в двух прочтениях. Текст, похожий на галлюцинацию на
/// тишине, до черновика не доходит — иначе посреди реплики выросло бы «Продолжение следует».
pub fn transcribe_chunk(
    engine: &mut dyn SttEngine,
    audio: &PcmAudio,
    options: &SttOptions,
    context: &ChunkContext,
    no_speech_threshold: f32,
) -> Result<ChunkText, SttError> {
    let options = chunk_options(options, context);
    let transcript = transcribe_with_language_policy(engine, audio, &options)?;
    let text = if is_silence_hallucination(&transcript, no_speech_threshold) {
        String::new()
    } else {
        transcript.text.trim().to_string()
    };
    Ok(ChunkText {
        text,
        language: language_of(&options, &transcript),
        stt_ms: 0,
    })
}

/// Параметры распознавания для куска: язык реплики и подсказка с хвостом уже сказанного.
pub fn chunk_options(options: &SttOptions, context: &ChunkContext) -> SttOptions {
    SttOptions {
        language: match &context.language {
            Some(code) => LanguageHint::Fixed(code.clone()),
            None => options.language.clone(),
        },
        initial_prompt: chunk_prompt(options.initial_prompt.as_deref(), &context.previous_text),
        ..options.clone()
    }
}

/// Параметры для хвоста реплики: тот же язык и тот же контекст, что и у последнего куска.
pub fn tail_options(options: &SttOptions, prefix: &ChunkPrefix) -> SttOptions {
    chunk_options(
        options,
        &ChunkContext {
            previous_text: tail_context(&prefix.text),
            language: prefix.language.clone(),
            index: usize::MAX,
        },
    )
}

/// Склеить начало реплики из кусков с распознанным хвостом.
pub fn merge(prefix: &ChunkPrefix, tail: Option<Transcript>) -> Transcript {
    let tail_text = tail
        .as_ref()
        .map(|t| t.text.trim().to_string())
        .unwrap_or_default();
    Transcript {
        text: join_texts(&[prefix.text.clone(), tail_text]),
        segments: Vec::new(),
        detected_language: prefix
            .language
            .clone()
            .or_else(|| tail.as_ref().and_then(|t| t.detected_language.clone())),
        // Речь в реплике уже нашлась в кусках: оценка тишины по одному хвосту выбросила бы её всю.
        no_speech_prob: None,
    }
}

/// Подсказка для куска: термины словаря плюс хвост уже распознанного текста.
pub fn chunk_prompt(dictionary_hint: Option<&str>, previous_text: &str) -> Option<String> {
    let previous = previous_text.trim();
    let hint = dictionary_hint.unwrap_or("").trim();
    let joined = match (hint.is_empty(), previous.is_empty()) {
        (true, true) => return None,
        (true, false) => previous.to_string(),
        (false, true) => hint.to_string(),
        (false, false) => format!("{hint}. {previous}"),
    };
    Some(joined)
}

/// Хвост текста не длиннее [`PROMPT_CONTEXT_CHARS`] символов, по границе слова.
pub fn tail_context(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= PROMPT_CONTEXT_CHARS {
        return text.to_string();
    }
    let skip = count - PROMPT_CONTEXT_CHARS;
    let tail: String = text.chars().skip(skip).collect();
    // Обрезать посередине слова незачем: подсказка из огрызка только сбивает модель.
    match tail.find(' ') {
        Some(space) => tail[space + 1..].to_string(),
        None => tail,
    }
}

/// Склейка текстов: пробел между кусками, пустые пропускаются.
fn join_texts(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join(" ")
}

/// Язык куска: что определила модель, а если она молчит — что просили.
fn language_of(options: &SttOptions, transcript: &Transcript) -> Option<String> {
    transcript
        .detected_language
        .clone()
        .or(match &options.language {
            LanguageHint::Fixed(code) => Some(code.clone()),
            LanguageHint::Auto => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::{FakeAudioSource, FakeStt};

    fn context(previous: &str, language: Option<&str>) -> ChunkContext {
        ChunkContext {
            previous_text: previous.into(),
            language: language.map(str::to_string),
            index: 1,
        }
    }

    fn audio(secs: f32) -> PcmAudio {
        PcmAudio::new(vec![0.2; (secs * 16_000.0) as usize], 16_000)
    }

    #[test]
    fn the_tail_of_the_previous_text_becomes_the_prompt_of_the_next_chunk() {
        let mut engine = FakeStt::returning("вторая часть");
        let options = SttOptions {
            initial_prompt: Some("MolvAI".into()),
            ..SttOptions::default()
        };

        transcribe_chunk(
            &mut engine,
            &audio(2.0),
            &options,
            &context("первая часть реплики", Some("ru")),
            0.6,
        )
        .expect("кусок распознан");

        let call = &engine.calls[0];
        let prompt = call.initial_prompt.as_deref().expect("подсказка есть");
        assert!(prompt.contains("MolvAI"), "потерялся словарь: {prompt}");
        assert!(
            prompt.contains("первая часть реплики"),
            "потерялся контекст предыдущего куска: {prompt}"
        );
        assert_eq!(
            call.language,
            LanguageHint::Fixed("ru".into()),
            "язык реплики выбирается один раз и держится до конца"
        );
    }

    #[test]
    fn a_chunk_that_is_only_a_hallucination_adds_nothing_to_the_draft() {
        let mut engine = FakeStt::returning("Продолжение следует...");
        let chunk = transcribe_chunk(
            &mut engine,
            &audio(2.0),
            &SttOptions::default(),
            &ChunkContext::default(),
            0.6,
        )
        .expect("кусок обработан");
        assert_eq!(
            chunk.text, "",
            "хвост субтитров попал бы в середину реплики"
        );
    }

    #[test]
    fn chunks_keep_their_order_and_are_joined_by_a_single_space() {
        let mut acc = ChunkAccumulator::default();
        acc.push(chunk_text("первый кусок", 100), 900);
        acc.push(chunk_text("", 100), 1_500);
        acc.push(chunk_text("второй кусок", 120), 2_000);

        assert_eq!(acc.draft(), "первый кусок второй кусок");
        let prefix = acc.take_prefix(7.5);
        assert_eq!(prefix.text, "первый кусок второй кусок");
        assert_eq!(prefix.stt_ms, 320, "время кусков суммируется");
        assert_eq!(
            prefix.first_hypothesis_ms,
            Some(900),
            "первая гипотеза — это первый кусок с текстом"
        );
        assert_eq!(prefix.audio_secs, Some(7.5));
        assert!(
            acc.is_empty(),
            "копилка обязана очиститься под новую реплику"
        );
    }

    #[test]
    fn the_language_of_the_reply_comes_from_the_first_chunk_with_speech() {
        let mut acc = ChunkAccumulator::default();
        acc.push(
            ChunkText {
                text: String::new(),
                language: Some("uk".into()),
                stt_ms: 10,
            },
            300,
        );
        acc.push(
            ChunkText {
                text: "привет".into(),
                language: Some("ru".into()),
                stt_ms: 10,
            },
            600,
        );
        acc.push(
            ChunkText {
                text: "hello".into(),
                language: Some("en".into()),
                stt_ms: 10,
            },
            900,
        );

        assert_eq!(acc.context().language.as_deref(), Some("ru"));
    }

    #[test]
    fn an_empty_accumulator_means_the_reply_is_processed_as_a_whole() {
        let acc = ChunkAccumulator::default();
        assert!(acc.is_empty());
        assert!(ChunkPrefix::default().is_empty());
        assert_eq!(acc.context(), ChunkContext::default());
    }

    #[test]
    fn the_prompt_context_is_cut_at_a_word_boundary() {
        let long = "слово ".repeat(100);
        let tail = tail_context(&long);
        assert!(tail.chars().count() <= PROMPT_CONTEXT_CHARS);
        assert!(
            tail.starts_with("слово"),
            "подсказка начинается с огрызка: {tail}"
        );
    }

    #[test]
    fn a_short_text_is_its_own_context() {
        assert_eq!(tail_context("  привет мир  "), "привет мир");
        assert_eq!(tail_context(""), "");
        assert_eq!(chunk_prompt(None, ""), None);
        assert_eq!(chunk_prompt(Some("MolvAI"), ""), Some("MolvAI".into()));
        assert_eq!(chunk_prompt(None, "текст"), Some("текст".into()));
    }

    #[test]
    fn the_tail_is_glued_to_the_chunks_and_keeps_their_language() {
        let prefix = ChunkPrefix {
            text: "начало реплики".into(),
            language: Some("ru".into()),
            ..ChunkPrefix::default()
        };
        let merged = merge(&prefix, Some(Transcript::text_only(" и хвост ")));

        assert_eq!(merged.text, "начало реплики и хвост");
        assert_eq!(merged.detected_language.as_deref(), Some("ru"));
        assert_eq!(
            merged.no_speech_prob, None,
            "речь уже нашлась в кусках: тишина в хвосте не отменяет реплику"
        );
    }

    #[test]
    fn a_reply_without_a_tail_is_just_its_chunks() {
        let prefix = ChunkPrefix {
            text: "вся реплика кусками".into(),
            ..ChunkPrefix::default()
        };
        assert_eq!(merge(&prefix, None).text, "вся реплика кусками");
    }

    #[test]
    fn the_feeder_cuts_the_stream_while_the_recording_is_still_running() {
        // Порции по восемь секунд: два тика — и в сегментаторе уже есть на что резать.
        let mut source = FakeAudioSource::paced(speech(14.0), 8_000);
        source.start(None).expect("запись началась");
        let mut feeder = ChunkFeeder::new(&SttConfig::default(), &AudioConfig::default());
        feeder.start(Instant::now());

        let first = feeder.tick(&mut source);
        let second = feeder.tick(&mut source);

        assert!(
            !first.is_empty() || !second.is_empty(),
            "за четырнадцать секунд речи не отдано ни куска: обработка так и ждёт отпускания"
        );
        let full = source.stop().expect("запись остановлена");
        let streamed = feeder.emitted();
        // Остаток после отпускания: хвоста может и не быть, если запись кончилась на паузе —
        // тишину распознавать незачем.
        let rest = feeder.finish(&full);

        assert!(streamed > 0);
        let seconds: f32 = first
            .iter()
            .chain(second.iter())
            .chain(rest.iter())
            .map(Chunk::duration_secs)
            .sum();
        assert!(
            seconds > 12.0,
            "куски покрывают не всю запись: {seconds:.1} с из четырнадцати"
        );
    }

    #[test]
    fn a_source_that_cannot_stream_yields_no_chunks() {
        let mut source = FakeAudioSource::silence(20.0);
        source.start(None).expect("запись началась");
        let mut feeder = ChunkFeeder::new(&SttConfig::default(), &AudioConfig::default());
        feeder.start(Instant::now());

        assert!(feeder.tick(&mut source).is_empty());
        assert_eq!(feeder.emitted(), 0);
        assert!(
            feeder.finish(&source.stop().unwrap()).is_empty(),
            "без сегментатора хвоста быть не может"
        );
    }

    #[test]
    fn switching_chunking_off_stops_the_feeder_entirely() {
        let stt = SttConfig {
            chunked: false,
            ..SttConfig::default()
        };
        let mut source = FakeAudioSource::paced(speech(20.0), 20_000);
        source.start(None).expect("запись началась");
        let mut feeder = ChunkFeeder::new(&stt, &AudioConfig::default());
        feeder.start(Instant::now());

        assert!(!feeder.is_enabled());
        assert!(feeder.tick(&mut source).is_empty());
        assert_eq!(feeder.emitted(), 0);
    }

    fn chunk_text(text: &str, stt_ms: u32) -> ChunkText {
        ChunkText {
            text: text.into(),
            language: Some("ru".into()),
            stt_ms,
        }
    }

    /// Речь с паузами: тон по полторы секунды через паузу в секунду.
    fn speech(secs: f32) -> PcmAudio {
        let rate = 16_000usize;
        let mut samples = Vec::new();
        while samples.len() < (secs * rate as f32) as usize {
            samples.extend((0..rate * 3 / 2).map(|i| 0.5 * (i as f32 * 0.3).sin()));
            samples.resize(samples.len() + rate, 0.0);
        }
        PcmAudio::new(samples, rate as u32)
    }
}
