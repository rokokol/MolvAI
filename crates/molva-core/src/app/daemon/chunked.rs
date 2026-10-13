// SPDX-License-Identifier: MIT
//! Streaming processing of a phrase: chunks go to the model while the person is still speaking
//!
//! Before, all the work started at the key release: a phrase of four seconds made the person
//! wait four more. Here the [`Segmenter`] cuts the recording during the speech, each ready chunk
//! goes to recognition at once, and after the release only the tail remains, usually less than a
//! second. The wait that the person sees shrinks to the last chunk plus post-processing
//!
//! The chunks go in one queue to one worker thread, so the order of the texts is the order of
//! the chunks, and they cannot get mixed up by construction. The context between chunks passes
//! as a hint: the `initial_prompt` of the next chunk is the tail of the text already recognized
//! together with the dictionary hint, so whisper keeps the style, the terms and the word forms
//!
//! The language is chosen by the first chunk of the phrase and applies to all the others.
//! Whisper does not split mixed speech inside one phrase anyway: it chooses one language for the
//! whole fragment, so a language change from chunk to chunk would give two different readings
//! of one phrase

use std::time::Instant;

use crate::config::{AudioConfig, SttConfig};
use crate::domain::audio::{AudioSource, PcmAudio};
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions, Transcript};
use crate::infra::stt::{is_silence_hallucination, transcribe_with_language_policy};

use crate::app::audio::segmenter::{Chunk, Segmenter, SegmenterConfig};

/// How many characters of the already recognized text go as a hint into the next chunk
///
/// Two hundred characters is about the last sentence: it is enough for the model to pick up the
/// style and the terms, and not so much that the hint starts to replace the sound itself
pub const PROMPT_CONTEXT_CHARS: usize = 200;

/// How often the daemon takes fresh sound from the microphone during recording
pub const TICK_MS: u64 = 250;

/// A recognized chunk of a phrase
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChunkText {
    /// An empty string means that the chunk held no speech
    pub text: String,
    /// Language in which the chunk was read
    pub language: Option<String>,
    pub stt_ms: u32,
}

/// What is known about the previous chunks when the pipeline takes the next one
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkContext {
    /// Tail of the already recognized text; it goes into `initial_prompt`
    pub previous_text: String,
    /// Language chosen by the first chunk of the phrase
    pub language: Option<String>,
    /// Number of the chunk, starting from zero
    pub index: usize,
}

/// Everything the chunks have collected: the start of the phrase for the pipeline
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkPrefix {
    pub text: String,
    pub language: Option<String>,
    /// Total recognition time of the chunks
    pub stt_ms: u32,
    /// From the start of recording to the first recognized chunk
    pub first_hypothesis_ms: Option<u32>,
    /// Duration of the whole phrase: the tail no longer knows it
    pub audio_secs: Option<f32>,
}

impl ChunkPrefix {
    /// An empty prefix means "there were no chunks": the phrase is recognized whole, as before
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Collector of the recognized chunks of one phrase
///
/// It lives in the worker thread of the daemon. That thread alone takes the chunk queue, so the
/// order holds without any numbers or sorting
#[derive(Debug, Default)]
pub struct ChunkAccumulator {
    texts: Vec<String>,
    language: Option<String>,
    stt_ms: u32,
    first_hypothesis_ms: Option<u32>,
}

impl ChunkAccumulator {
    /// No chunk with text has been recognized yet
    pub fn is_empty(&self) -> bool {
        self.texts.iter().all(|text| text.trim().is_empty())
    }

    /// Context for the next chunk
    pub fn context(&self) -> ChunkContext {
        ChunkContext {
            previous_text: tail_context(&self.draft()),
            language: self.language.clone(),
            index: self.texts.len(),
        }
    }

    /// Accept a recognized chunk; `since_start_ms` is the time since the start of recording
    pub fn push(&mut self, chunk: ChunkText, since_start_ms: u32) {
        self.stt_ms = self.stt_ms.saturating_add(chunk.stt_ms);
        if self.language.is_none() {
            // The language of the phrase is chosen by the first chunk that held speech
            if !chunk.text.trim().is_empty() {
                self.language.clone_from(&chunk.language);
            }
        }
        if self.first_hypothesis_ms.is_none() && !chunk.text.trim().is_empty() {
            self.first_hypothesis_ms = Some(since_start_ms);
        }
        self.texts.push(chunk.text);
    }

    /// Draft of the whole phrase: what the person sees during speech
    pub fn draft(&self) -> String {
        join_texts(&self.texts)
    }

    /// Take what was collected and clear for the next phrase
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

    /// Forget what was collected: the recording was cancelled
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Cutter on the side of the control thread: takes fresh sound and returns ready chunks
#[derive(Debug)]
pub struct ChunkFeeder {
    enabled: bool,
    pause_ms: u32,
    silence_threshold_db: f32,
    segmenter: Option<Segmenter>,
    /// How many samples already went into the segmenter
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

    /// Start of a new recording: the collector resets
    pub fn start(&mut self, at: Instant) {
        self.segmenter = None;
        self.fed = 0;
        self.started = at;
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    /// How many chunks were returned since the start of the recording
    pub fn emitted(&self) -> usize {
        self.segmenter.as_ref().map_or(0, Segmenter::emitted)
    }

    /// Take fresh sound and collect what has matured into a chunk
    pub fn tick(&mut self, source: &mut dyn AudioSource) -> Vec<Chunk> {
        if !self.enabled {
            return Vec::new();
        }
        let Some(fresh) = source.drain_new_samples() else {
            // The source cannot return sound during recording: the phrase will go whole
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

    /// Tail of the recording: the last chunk and everything the tick did not take
    ///
    /// The last element is the tail, and the others are ordinary chunks. If the person finished
    /// a whole sentence between the last tick and the key release, the remainder may hold several
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

    /// The recording was cancelled: the segmenter is no longer needed
    pub fn cancel(&mut self) {
        self.segmenter = None;
        self.fed = 0;
    }
}

/// Recognize one chunk of a phrase
///
/// The language is fixed by the first chunk: whisper reads a fragment in one language anyway,
/// and a language that jumps from chunk to chunk would give one sentence in two readings.
/// Text that looks like a hallucination on silence does not reach the draft. Otherwise
/// "Продолжение следует" would grow in the middle of a phrase
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

/// Recognition parameters for a chunk: the phrase language and a hint with the tail of what
/// was already said
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

/// Parameters for the tail of a phrase: the same language and the same context as for the last
/// chunk
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

/// Join the start of a phrase, made of chunks, with the recognized tail
pub fn merge(prefix: &ChunkPrefix, tail: Option<Transcript>) -> Transcript {
    let (tail_text, tail_language) = match tail {
        Some(tail) => (tail.text.trim().to_string(), tail.detected_language),
        None => (String::new(), None),
    };
    Transcript {
        text: join_texts(&[prefix.text.clone(), tail_text]),
        segments: Vec::new(),
        detected_language: prefix.language.clone().or(tail_language),
        // The speech of the phrase was already found in the chunks: a silence estimate on the
        // tail alone would discard it all
        no_speech_prob: None,
    }
}

/// Hint for a chunk: the dictionary terms plus the tail of the already recognized text
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

/// Tail of a text no longer than [`PROMPT_CONTEXT_CHARS`] characters, cut at a word boundary
pub fn tail_context(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= PROMPT_CONTEXT_CHARS {
        return text.to_string();
    }
    let skip = count - PROMPT_CONTEXT_CHARS;
    let tail: String = text.chars().skip(skip).collect();
    // There is no reason to cut in the middle of a word: a hint made of a stump only confuses
    // the model
    match tail.find(' ') {
        Some(space) => tail[space + 1..].to_string(),
        None => tail,
    }
}

/// Join of texts: a space between chunks, and empty ones are skipped
fn join_texts(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join(" ")
}

/// Language of a chunk: what the model detected, and if it is silent, what was requested
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
        .expect("the chunk is recognized");

        let call = &engine.calls[0];
        let prompt = call.initial_prompt.as_deref().expect("there is a hint");
        assert!(
            prompt.contains("MolvAI"),
            "the dictionary was lost: {prompt}"
        );
        assert!(
            prompt.contains("первая часть реплики"),
            "the context of the previous chunk was lost: {prompt}"
        );
        assert_eq!(
            call.language,
            LanguageHint::Fixed("ru".into()),
            "the phrase language is chosen once and holds to the end"
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
        .expect("the chunk is processed");
        assert_eq!(
            chunk.text, "",
            "a subtitle tail would land in the middle of a phrase"
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
        assert_eq!(prefix.stt_ms, 320, "the times of the chunks add up");
        assert_eq!(
            prefix.first_hypothesis_ms,
            Some(900),
            "the first hypothesis is the first chunk with text"
        );
        assert_eq!(prefix.audio_secs, Some(7.5));
        assert!(acc.is_empty(), "the collector must clear for a new phrase");
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
            "the hint starts with a stump: {tail}"
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
            "the speech was already found in chunks: silence in the tail does not cancel the phrase"
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
        // Portions of eight seconds: two ticks, and the segmenter already has something to cut
        let mut source = FakeAudioSource::paced(speech(14.0), 8_000);
        source.start(None).expect("recording started");
        let mut feeder = ChunkFeeder::new(&SttConfig::default(), &AudioConfig::default());
        feeder.start(Instant::now());

        let first = feeder.tick(&mut source);
        let second = feeder.tick(&mut source);

        assert!(
            !first.is_empty() || !second.is_empty(),
            "no chunk returned in fourteen seconds of speech: processing still waits for the release"
        );
        let full = source.stop().expect("recording stopped");
        let streamed = feeder.emitted();
        // The remainder after the release: there may be no tail if the recording ended on a
        // pause, because there is no reason to recognize silence
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
            "the chunks do not cover the whole recording: {seconds:.1} s of fourteen"
        );
    }

    #[test]
    fn a_source_that_cannot_stream_yields_no_chunks() {
        let mut source = FakeAudioSource::silence(20.0);
        source.start(None).expect("recording started");
        let mut feeder = ChunkFeeder::new(&SttConfig::default(), &AudioConfig::default());
        feeder.start(Instant::now());

        assert!(feeder.tick(&mut source).is_empty());
        assert_eq!(feeder.emitted(), 0);
        assert!(
            feeder.finish(&source.stop().unwrap()).is_empty(),
            "without a segmenter there can be no tail"
        );
    }

    #[test]
    fn switching_chunking_off_stops_the_feeder_entirely() {
        let stt = SttConfig {
            chunked: false,
            ..SttConfig::default()
        };
        let mut source = FakeAudioSource::paced(speech(20.0), 20_000);
        source.start(None).expect("recording started");
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

    /// Speech with pauses: a tone of one and a half seconds, then a pause of one second
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
