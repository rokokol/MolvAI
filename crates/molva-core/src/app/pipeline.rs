// SPDX-License-Identifier: MIT
//! Pipeline of one phrase: audio → recognition → dictionary → rules → model → paste → journal
//!
//! The main property of the pipeline is that a phrase cannot be lost. The model did not answer,
//! the key expired, the server went down: the text after the rules still reaches the paste step,
//! the journal holds `llm_used = false`, and the log holds a warning. A paste error does not
//! fail `run` either: it goes to `Entry.error`, and the text stays in the clipboard through the
//! effort of the paste implementation itself
//!
//! The model is called only when all conditions hold at once: the style needs it, post-processing
//! is on, privacy allows it, and the phrase reaches the word minimum of that style. Short
//! phrases go through the rules only, which is both faster and cheaper in tokens
//!
//! A snippet is checked after the dictionary and the rules, before the model. A whole phrase that
//! matches a snippet returns its replacement at once: no style, no model and no tokens
//!

use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::warn;
use uuid::Uuid;

use crate::app::command_mode;
use crate::app::snippets::SnippetSet;
use crate::config::{
    AudioConfig, CommandModeConfig, Config, DictionaryConfig, LlmConfig, OutputConfig,
    PrivacyConfig, RulesConfig, SnippetsConfig, SttConfig, StyleConfig,
};
use crate::domain::audio::PcmAudio;
use crate::domain::clock::Clock;
use crate::domain::entry::{Entry, LatencyMs, Mode, Source, Tokens, SCHEMA_VERSION};
use crate::domain::inject::{OutputMode, TextInjector};
use crate::domain::journal::{Journal, JournalError};
use crate::domain::llm::{ChatRequest, LlmClient, LlmError};
use crate::domain::stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};
use crate::domain::text::word_count;
use crate::ipc::protocol::{FileFailure, FileReport, FileTranscriptionRequest};

use super::daemon::chunked::{self, ChunkContext, ChunkPrefix, ChunkText};
use super::dictionary::Dictionary;
use super::file_result::FileResult;
use super::llm_executor::LlmExecutor;
use super::llm_output::sanitize_llm_output;
use super::rules::RuleSet;
use super::styles::Styles;
use crate::infra::audio::decode;
use crate::infra::stt::{is_silence_hallucination, transcribe_with_language_policy};

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("recognition failed: {0}")]
    Stt(#[from] SttError),
    #[error("journal not available: {0}")]
    Journal(#[from] JournalError),
    #[error("cannot get the selected text: {0}")]
    Selection(String),
    #[error("voice editing is turned off in the settings")]
    CommandModeDisabled,
    #[error("voice editing needs a post-processing model: {0}")]
    CommandModeNeedsLlm(LlmError),
    #[error("paste failed: {0}")]
    Inject(#[from] crate::domain::inject::InjectError),
    #[error("the transcription was cancelled")]
    Cancelled,
}

/// What one call to the model spent, whatever came back
///
/// A retry is a request the provider bills for just like the first one, so the money spent on an
/// answer that was thrown away stays in this record. The counters travel with the money, because
/// a sum is only honest beside the number of calls it covers and the number of those that named
/// no price
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct LlmSpend {
    /// Sum of the charges the provider reported, in US dollars
    cost_usd: Option<f64>,
    /// Requests that left for the provider, including the ones whose answer was thrown away
    calls: u32,
    /// Calls that came back without a charge: the provider does not account, or the request
    /// never reached it
    unpriced: u32,
}

impl LlmSpend {
    fn add(&mut self, cost_usd: Option<f64>) {
        self.calls += 1;
        match cost_usd {
            Some(cost) => self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost),
            None => self.unpriced += 1,
        }
    }
}

/// What one successful call to the model produced
///
/// The four facts travel together because the journal records them together, and a tuple of four
/// would let a caller swap the tokens for the charge without the compiler noticing
#[derive(Debug, Clone, PartialEq)]
struct LlmOutcome {
    text: String,
    tokens: Option<Tokens>,
    elapsed_ms: u32,
}

/// Where a chain of attempts ended, with the money spent on the way
///
/// The spend is present either way, because a phrase that ends in a failure still cost what the
/// provider charged for the attempts it did answer
#[derive(Debug)]
struct LlmAttempt {
    outcome: Result<LlmOutcome, LlmError>,
    spend: LlmSpend,
}

/// Settings that the pipeline needs. They are built from [`Config`] in one call
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub audio: AudioConfig,
    pub stt: SttConfig,
    pub dictionary: DictionaryConfig,
    pub rules: RulesConfig,
    pub snippets: SnippetsConfig,
    pub llm: LlmConfig,
    pub style: StyleConfig,
    pub output: OutputConfig,
    pub command_mode: CommandModeConfig,
    pub privacy: PrivacyConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl PipelineConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            audio: cfg.audio.clone(),
            stt: cfg.stt.clone(),
            dictionary: cfg.dictionary.clone(),
            rules: cfg.rules.clone(),
            snippets: cfg.snippets.clone(),
            llm: cfg.llm.clone(),
            style: cfg.style.clone(),
            output: cfg.output.clone(),
            command_mode: cfg.command_mode.clone(),
            privacy: cfg.privacy.clone(),
        }
    }
}

/// Parsing of `output.mode` from the settings; an unknown value gives `auto`
fn output_mode(value: &str) -> OutputMode {
    match value.trim().to_lowercase().as_str() {
        "paste" => OutputMode::Paste,
        "type" => OutputMode::Type,
        "clipboard" => OutputMode::Clipboard,
        _ => OutputMode::Auto,
    }
}

/// Paste method as it goes to the journal
///
/// Criterion AJ-09/AJ-10: if there is no active window, there is nowhere to paste. The text
/// stays in the clipboard, the paste implementation tells the user with a notification, and the
/// journal shows the case separately (`clipboard-no-focus`), and does not pass it off as a
/// successful paste into an input field
pub const NO_FOCUS_METHOD: &str = "clipboard-no-focus";

/// The journal method of a phrase whose chunks were typed while the person spoke
pub const STREAMED_METHOD: &str = "streamed";

fn inject_method_for(method: &str, app_hint: Option<&str>) -> String {
    if app_hint.is_none() && method == "clipboard-only" {
        return NO_FOCUS_METHOD.to_string();
    }
    method.to_string()
}

/// The window that will receive the text: the one focused now, not the one that was focused
/// when the key went down
///
/// A phrase takes time to recognize and to process, and the user may switch while it runs. The
/// class decides only the paste shortcut, but a terminal with Ctrl+V or a browser with
/// Ctrl+Shift+V silently receives nothing, so the class is asked again right before the paste
fn target_window(app_hint: Option<&str>) -> Option<String> {
    crate::infra::platform::active_window_class().or_else(|| app_hint.map(str::to_string))
}

fn millis_since(start: Instant, now: Instant) -> u32 {
    now.saturating_duration_since(start)
        .as_millis()
        .min(u128::from(u32::MAX)) as u32
}

/// The part of the phrase that the chunks did not cover
fn tail_after_prefix<'a>(full: &'a str, prefix: &str) -> &'a str {
    full.strip_prefix(prefix).map_or(full, str::trim_start)
}

/// What the field got from one chunk during the recording
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamProgress {
    /// The whole text typed into the field so far, with the separators between the chunks
    pub typed: String,
}

/// Whether two pieces need a space between them. Punctuation attaches to the word before it
fn needs_space(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    !right
        .chars()
        .next()
        .is_some_and(|ch| ",.!?;:…)]}".contains(ch))
}

/// Glue two pieces of text with the separator their first characters ask for
fn glue(left: &str, right: &str) -> String {
    let separator = if needs_space(left, right) { " " } else { "" };
    format!("{left}{separator}{right}")
}

/// Pipeline of one phrase
#[derive(Debug)]
pub struct Pipeline {
    stt: Box<dyn SttEngine>,
    /// The model under the policy of [`LlmExecutor`]: one local call at a time, cloud calls
    /// beside each other. Every stage of the pipeline goes through this one object
    llm: Option<Arc<LlmExecutor>>,
    injector: Box<dyn TextInjector>,
    journal: Box<dyn Journal>,
    clock: Arc<dyn Clock>,
    config: PipelineConfig,
    dictionary: Dictionary,
    rules: RuleSet,
    snippets: SnippetSet,
    styles: Styles,
    session_id: Uuid,
    /// Time from the key release to the close of the microphone stream: the daemon measures
    /// it, and the pipeline records it
    stop_after_release_ms: Option<u32>,
    /// Start of the phrase, recognized in chunks during recording; `None` means an ordinary
    /// whole phrase
    chunk_prefix: Option<ChunkPrefix>,
    /// Text already typed while the person spoke, with the separators between chunks
    streamed: String,
    /// Dictionary hits of the typed chunks, for the journal
    streamed_dict_hits: u32,
    /// Time spent in the chunk pastes
    streamed_inject_ms: u32,
    /// A chunk paste failed: the typed prefix stays, and the phrase is not retyped from the start
    streamed_failed: bool,
    /// The job a client may give up on: the flag and the process of the model, so a cancel
    /// stops the model itself rather than only the next step
    file_cancel: Option<Arc<crate::domain::cancel::CancelToken>>,
}

impl Pipeline {
    pub fn new(
        stt: Box<dyn SttEngine>,
        llm: Option<Arc<dyn LlmClient>>,
        injector: Box<dyn TextInjector>,
        journal: Box<dyn Journal>,
        clock: Arc<dyn Clock>,
        config: PipelineConfig,
    ) -> Self {
        let rules = RuleSet::from_config(&config.rules);
        let snippets = SnippetSet::from_config(&config.snippets);
        let styles = Styles::from_config(&config.style);
        Self {
            stt,
            // The client arrives ready to talk, and the policy of who may wait for whom is not
            // the business of a caller: the executor is built here, once
            llm: LlmExecutor::wrap(llm),
            injector,
            journal,
            clock,
            config,
            dictionary: Dictionary::empty(),
            rules,
            snippets,
            styles,
            session_id: Uuid::new_v4(),
            stop_after_release_ms: None,
            chunk_prefix: None,
            streamed: String::new(),
            streamed_dict_hits: 0,
            streamed_inject_ms: 0,
            streamed_failed: false,
            file_cancel: None,
        }
    }

    /// Give the pipeline the flag that a client sets to stop the file transcription
    /// Give the pipeline the token that a client cancels: the engine takes it too, so the
    /// process of the recognizer dies with the job
    pub fn set_file_cancel(&mut self, cancel: Arc<crate::domain::cancel::CancelToken>) {
        self.file_cancel = Some(Arc::clone(&cancel));
        self.stt.set_cancel(cancel);
    }

    /// Whether a client has given up on the file that is being transcribed
    fn file_cancelled(&self) -> bool {
        self.file_cancel
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    /// Record the daemon measurement: how long the microphone took to free after the key release
    ///
    /// One phrase consumes the value: the next phrase gets its own measurement, not the old one
    pub fn set_stop_after_release(&mut self, ms: u32) {
        self.stop_after_release_ms = Some(ms);
    }

    /// Attach the dictionary of terms
    #[must_use]
    pub fn with_dictionary(mut self, dictionary: Dictionary) -> Self {
        self.dictionary = dictionary;
        self
    }

    pub fn set_dictionary(&mut self, dictionary: Dictionary) {
        self.dictionary = dictionary;
    }

    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// Reread the dictionary if the file changed
    pub fn reload_dictionary(&mut self) -> bool {
        match self.dictionary.reload_if_changed() {
            Ok(changed) => changed,
            Err(err) => {
                warn!(error = %err, "dictionary not reread, the old one stays");
                false
            }
        }
    }

    /// Apply new settings: the rules and the styles are rebuilt
    pub fn set_config(&mut self, config: PipelineConfig) {
        self.rules = RuleSet::from_config(&config.rules);
        self.snippets = SnippetSet::from_config(&config.snippets);
        self.styles = Styles::from_config(&config.style);
        self.config = config;
    }

    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    pub fn styles(&self) -> &Styles {
        &self.styles
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn set_session_id(&mut self, session_id: Uuid) {
        self.session_id = session_id;
    }

    /// Run a phrase through the whole pipeline
    ///
    /// The recording comes by value: from here on it belongs to the pipeline, and the caller has
    /// no reason to keep a second such buffer in memory
    #[allow(clippy::needless_pass_by_value)]
    pub fn run(
        &mut self,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, PipelineError> {
        let result = self.run_phrase(audio, mode, style, app_hint);
        // The phrase is over, however it ended: what it typed must not reach the next phrase
        self.discard_stream();
        result
    }

    /// Transcribe one audio or video file with the recognizer and the post-processing of this
    /// pipeline
    ///
    /// A file is not a phrase: nothing is pasted, the microphone takes no part, and the text goes
    /// back to the caller as a [`FileResult`]. Everything else is the same instance the daemon
    /// already holds — the recognizer with its weights, the language, the dictionary, the rules
    /// and the model — so a file gets the text a dictation of the same sound would get. A failed
    /// file is a line of the report, not a lost batch: the caller sees which input failed and why
    ///
    pub fn transcribe_file(&mut self, request: &FileTranscriptionRequest) -> FileReport {
        let mut report = FileReport::default();
        // The daemon has its own working directory: a relative path would name a different file
        // there, so the label and the read go by the absolute one
        let path = match std::fs::canonicalize(&request.path) {
            Ok(path) => path,
            Err(err) => {
                report.errors.push(FileFailure {
                    file: request.path.display().to_string(),
                    reason: format!("cannot read the file: {err}"),
                });
                return report;
            }
        };
        let label = path.display().to_string();
        let audio = match decode::decode_file(&path) {
            Ok(audio) => audio,
            Err(err) => {
                report.errors.push(FileFailure {
                    file: label,
                    reason: err.to_string(),
                });
                return report;
            }
        };
        match self.transcribe_audio(label, audio, request) {
            Ok(result) => report.results.push(result),
            Err(reason) => report.errors.push(FileFailure {
                file: request.path.display().to_string(),
                reason,
            }),
        }
        report
    }

    /// Run already decoded audio through the file path: decoding is the only difference
    /// between a file on disk and a stream on standard input
    #[allow(clippy::needless_pass_by_value)]
    pub fn transcribe_audio(
        &mut self,
        label: String,
        audio: PcmAudio,
        request: &FileTranscriptionRequest,
    ) -> Result<FileResult, String> {
        let audio_secs = audio.duration_secs();
        let ready = audio.to_16k();
        let started = self.clock.instant();
        let options = self.stt_options(request.timecodes);
        let transcript = self
            .transcribe_ready(&ready, &options, request.chunked)
            .map_err(|err| err.to_string())?;
        let stt_ms = millis_since(started, self.clock.instant());

        // The same guard as for a phrase: whisper prints a confident phrase on silence, and such
        // a text is not a transcript of the file
        let text_raw = if is_silence_hallucination(&transcript, self.config.stt.no_speech_threshold)
        {
            String::new()
        } else {
            transcript.text.trim().to_string()
        };
        let language = transcript
            .detected_language
            .clone()
            .or_else(|| match &options.language {
                LanguageHint::Fixed(code) => Some(code.clone()),
                LanguageHint::Auto => None,
            });
        let lang = language.clone().unwrap_or_else(|| "auto".to_string());
        let style = self
            .styles
            .resolve(request.style.as_deref(), None, &self.config.style);

        let rules_started = self.clock.instant();
        let (after_dictionary, dict_hits) = self.dictionary.apply(&text_raw);
        let after_rules = self.rules.apply(&after_dictionary, &lang);
        let rules_ms = millis_since(rules_started, self.clock.instant());

        let mut entry = Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            ts: self.clock.now_utc(),
            session_id: self.session_id,
            mode: Mode::Dictation,
            source: request.source,
            snippet: false,
            app: None,
            language: language.clone(),
            audio_secs,
            words: 0,
            wpm: None,
            style: style.id.clone(),
            stt_engine: self.stt.id().to_string(),
            stt_model: self.stt.model_name().to_string(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: self.llm.as_ref().is_none_or(|llm| llm.is_local()),
            dict_hits,
            inject_method: None,
            latency_ms: LatencyMs {
                stt: stt_ms,
                rules: rules_ms,
                ..Default::default()
            },
            tokens: None,
            llm_cost_usd: None,
            llm_calls: 0,
            llm_unpriced_calls: 0,
            error: None,
            text_raw: Some(text_raw),
            text_final: None,
            audio_path: Some(label.clone()),
            segments: transcript.segments.clone(),
        };

        let text = if request.no_llm {
            after_rules
        } else {
            self.run_dictation(&mut entry, &style, after_rules)
        };
        entry.words = word_count(&text);
        entry.wpm = Entry::wpm_for(entry.words, audio_secs);
        entry.text_final = Some(text.clone());
        entry.latency_ms.total = millis_since(started, self.clock.instant());

        // The journal is an auxiliary function here as in the CLI: its failure must not cost the
        // caller the text. A file transcription is a file in the journal, so search finds it
        if !self.config.privacy.no_record_mode {
            if let Err(err) = self.journal.append(&entry) {
                warn!(error = %err, file = %label, "the file transcription was not written to the journal");
            }
        }

        Ok(FileResult {
            file: label,
            text,
            segments: transcript.segments,
            audio_secs,
            latency_ms: entry.latency_ms.total,
            language,
        })
    }

    /// Recognize a whole file, optionally cut at the same pauses as a live phrase
    fn transcribe_ready(
        &mut self,
        ready: &PcmAudio,
        options: &SttOptions,
        chunked: bool,
    ) -> Result<Transcript, SttError> {
        if !chunked {
            if self.file_cancelled() {
                return Err(SttError::Inference("cancelled".into()));
            }
            return transcribe_with_language_policy(self.stt.as_mut(), ready, options);
        }
        let config = crate::app::audio::segmenter::SegmenterConfig::new(
            ready.sample_rate,
            self.config.stt.chunk_pause_ms,
            self.config.audio.silence_threshold_db,
        );
        // Neighbouring chunks share this much sound, so the halves of the overlap belong to the
        // chunk on their own side: the segments then meet instead of covering each other
        let half_overlap = config.overlap_ms / 2;
        let mut segmenter = crate::app::audio::segmenter::Segmenter::new(config);
        let mut chunks = segmenter.push(&ready.samples);
        chunks.extend(segmenter.finish());
        if chunks.is_empty() {
            return Ok(Transcript::default());
        }

        let mut text = Vec::new();
        let mut segments = Vec::new();
        let mut detected_language = None;
        for chunk in chunks {
            // The client may give up while the model works on the previous chunk: the next one is
            // not started, and the whole job answers as cancelled
            if self.file_cancelled() {
                return Err(SttError::Inference("cancelled".into()));
            }
            let transcript =
                transcribe_with_language_policy(self.stt.as_mut(), &chunk.audio, options)?;
            if detected_language.is_none() {
                detected_language.clone_from(&transcript.detected_language);
            }
            if is_silence_hallucination(&transcript, self.config.stt.no_speech_threshold) {
                continue;
            }
            let trimmed = transcript.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            text.push(trimmed.to_string());
            // One segment per chunk: the chunk boundaries are the pauses this file was cut at,
            // and the inner split of the model is finer than the audio it saw, so its own
            // segments come out shorter than a second and drift from the sound
            let start_ms = chunk.start_ms;
            let end_ms = start_ms.saturating_add((chunk.duration_secs() * 1000.0).round() as u32);
            let (start_ms, end_ms) = if start_ms == 0 {
                (start_ms, end_ms.saturating_sub(half_overlap))
            } else {
                (
                    start_ms.saturating_add(half_overlap),
                    end_ms.saturating_sub(half_overlap),
                )
            };
            segments.push(Segment {
                start_ms,
                end_ms: end_ms.max(start_ms + 1),
                text: format!(" {trimmed}"),
            });
        }
        Ok(Transcript {
            text: text.join(" "),
            segments,
            detected_language,
            no_speech_prob: None,
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    fn run_phrase(
        &mut self,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, PipelineError> {
        let started = self.clock.instant();
        let audio = audio.to_16k();
        // Chunks recognized during recording; the input here is then only the tail of the
        // phrase, and the prefix knows its duration
        let prefix = self.chunk_prefix.take().unwrap_or_default();
        let streamed = self.streamed.clone();
        let streamed_hits = std::mem::take(&mut self.streamed_dict_hits);
        let streamed_ms = std::mem::take(&mut self.streamed_inject_ms);
        self.streamed_failed = false;
        let streaming = !streamed.is_empty();
        let audio_secs = prefix.audio_secs.unwrap_or_else(|| audio.duration_secs());

        // The selection is taken before recognition: while the user spoke, it was still in place
        let selection = match mode {
            Mode::Command => {
                if !self.config.command_mode.enabled {
                    return Err(PipelineError::CommandModeDisabled);
                }
                Some(
                    self.injector
                        .copy_selection()
                        .map_err(|err| PipelineError::Selection(err.to_string()))?,
                )
            }
            Mode::Dictation => None,
        };

        let style = self.styles.resolve(style, app_hint, &self.config.style);

        let options = self.stt_options(false);
        let stt_started = self.clock.instant();
        let transcript = self.transcribe_utterance(&audio, &options, &prefix)?;
        let stt_ms = millis_since(stt_started, self.clock.instant()) + prefix.stt_ms;

        // Silence and noise: whisper confidently prints "Продолжение следует" on an empty input.
        // Such a phrase is neither pasted nor given to the model, but it enters the journal (F-22)
        let text_raw = if is_silence_hallucination(&transcript, self.config.stt.no_speech_threshold)
        {
            String::new()
        } else {
            transcript.text.trim().to_string()
        };
        let language = transcript
            .detected_language
            .clone()
            .or_else(|| match &options.language {
                LanguageHint::Fixed(code) => Some(code.clone()),
                LanguageHint::Auto => None,
            });
        let lang = language.clone().unwrap_or_else(|| "auto".to_string());

        let rules_started = self.clock.instant();
        let dict_hits;
        let mut streamed_tail = String::new();
        let after_rules = match mode {
            // The voice editing instruction goes to the model as it is: the rules would distort
            // the command
            Mode::Command => {
                let (after_dictionary, hits) = self.dictionary.apply(&text_raw);
                dict_hits = hits;
                after_dictionary
            }
            // The prefix is already in the field with its own rules applied. Only the tail is
            // processed now, so the two halves cannot disagree at their boundary
            Mode::Dictation if streaming => {
                let tail_raw = tail_after_prefix(&text_raw, &prefix.text);
                let (tail_dictionary, hits) = self.dictionary.apply(tail_raw);
                dict_hits = hits;
                streamed_tail = self.rules.apply(&tail_dictionary, &lang).trim().to_string();
                glue(&streamed, &streamed_tail)
            }
            Mode::Dictation => {
                let (after_dictionary, hits) = self.dictionary.apply(&text_raw);
                dict_hits = hits;
                self.rules.apply(&after_dictionary, &lang)
            }
        };
        let rules_ms = millis_since(rules_started, self.clock.instant());

        let mut entry = Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            ts: self.clock.now_utc(),
            session_id: self.session_id,
            mode,
            source: Source::Mic,
            snippet: false,
            app: app_hint.map(ToString::to_string),
            language,
            audio_secs,
            words: 0,
            wpm: None,
            style: style.id.clone(),
            stt_engine: self.stt.id().to_string(),
            stt_model: self.stt.model_name().to_string(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: self.llm.as_ref().is_none_or(|llm| llm.is_local()),
            dict_hits: dict_hits.saturating_add(streamed_hits),
            inject_method: None,
            latency_ms: LatencyMs {
                stt: stt_ms,
                rules: rules_ms,
                stop_after_release: self.stop_after_release_ms.take(),
                first_hypothesis: prefix.first_hypothesis_ms,
                ..Default::default()
            },
            tokens: None,
            llm_cost_usd: None,
            llm_calls: 0,
            llm_unpriced_calls: 0,
            error: None,
            text_raw: Some(text_raw),
            text_final: None,
            audio_path: None,
            segments: Vec::new(),
        };

        let tail_typed = streamed_tail.clone();
        let text = match mode {
            Mode::Command => {
                let selection = selection.unwrap_or_default();
                self.run_command(&mut entry, &selection, &after_rules)?
            }
            // The text of the prefix is already in the user's field: a model answer on the
            // whole phrase would rewrite what cannot be rewritten
            Mode::Dictation if streaming => after_rules,
            // A snippet replaces the whole phrase and returns before the model: a prepared
            // answer must not wait for a call and must not be rewritten by a style
            Mode::Dictation => match self.snippets.replacement(&after_rules) {
                Some(replacement) => {
                    entry.snippet = true;
                    replacement.to_string()
                }
                None => self.run_dictation(&mut entry, &style, after_rules),
            },
        };

        entry.words = word_count(&text);
        entry.wpm = Entry::wpm_for(entry.words, audio_secs);
        entry.text_final = Some(text.clone());

        self.deliver(
            &mut entry,
            &text,
            &tail_typed,
            streaming,
            streamed_ms,
            app_hint,
        );

        entry.latency_ms.total = millis_since(started, self.clock.instant());

        if self.config.privacy.no_record_mode {
            // The "do not record" mode: the journal is not touched at all
            return Ok(entry);
        }
        self.journal.append(&entry)?;
        Ok(entry)
    }

    /// Put the finished text into the field
    ///
    /// After a streamed phrase the field already holds the prefix, so only `tail` is still
    /// missing; otherwise the whole phrase goes in one paste
    fn deliver(
        &mut self,
        entry: &mut Entry,
        text: &str,
        tail: &str,
        streaming: bool,
        streamed_ms: u32,
        app_hint: Option<&str>,
    ) {
        if text.trim().is_empty() {
            // Nothing to paste: silence or a hallucination on empty audio
            entry.error = Some("empty phrase: nothing to paste".into());
            warn!("the phrase is empty after post-processing, paste skipped");
        } else if streaming {
            // The prefix is already in the field. Only the tail after it is still missing; a
            // leading space keeps the last typed word and the tail apart
            let prefix = self.streamed.clone();
            if !tail.is_empty() {
                self.inject_tail(entry, app_hint, &prefix, tail);
            }
            if entry.error.is_none() {
                entry.inject_method = Some(STREAMED_METHOD.to_string());
            }
            let streamed_ms = streamed_ms.saturating_add(self.streamed_inject_ms);
            entry.latency_ms.inject = Some(
                entry
                    .latency_ms
                    .inject
                    .map_or(streamed_ms, |tail| streamed_ms.saturating_add(tail)),
            );
        } else {
            let resolved = output_mode(&self.config.output.mode)
                .resolve(text, self.config.output.auto_type_max_chars as usize);
            self.wait_before_inject();
            let target = target_window(app_hint);
            let target = target.as_deref();
            // The class decides the paste shortcut: a terminal needs Ctrl+Shift+V, and a
            // browser needs Ctrl+V. Ask after the focus settles, not at the key press
            self.injector.set_window(target);
            let inject_started = self.clock.instant();
            match self.injector.inject(text, resolved) {
                Ok(report) => {
                    entry.inject_method = Some(inject_method_for(&report.method, target));
                    entry.latency_ms.inject =
                        Some(millis_since(inject_started, self.clock.instant()));
                }
                Err(err) => {
                    warn!(error = %err, "paste failed, the text stays in the clipboard");
                    entry.error = Some(err.to_string());
                }
            }
        }
    }

    /// Pause before the paste, set by the setting `output.pre_inject_delay_ms`
    ///
    /// Between the key release and the paste, the window must have time to return focus to the
    /// input field. Locally the default of 50 ms is enough. On a remote desktop and in slow
    /// applications the pause is set by hand, up to 1500 ms, or the first characters go nowhere
    fn wait_before_inject(&self) {
        let delay = self.config.output.pre_inject_delay_ms;
        if delay == 0 {
            return;
        }
        self.clock.sleep(Duration::from_millis(u64::from(delay)));
    }

    /// Recognize a chunk of a recording that is still running (streaming processing, see
    /// [`chunked`])
    ///
    /// The daemon collects the chunks, and the pipeline only reads them with the model, with the
    /// dictionary, the phrase language and the tail of the previous text in the hint
    pub fn transcribe_chunk(
        &mut self,
        audio: &PcmAudio,
        context: &ChunkContext,
    ) -> Result<ChunkText, SttError> {
        let options = self.stt_options(false);
        let started = self.clock.instant();
        let mut chunk = chunked::transcribe_chunk(
            self.stt.as_mut(),
            &audio.to_16k(),
            &options,
            context,
            self.config.stt.no_speech_threshold,
        )?;
        chunk.stt_ms = millis_since(started, self.clock.instant());
        Ok(chunk)
    }

    /// Give the pipeline the start of the phrase, recognized in chunks: the next
    /// [`Pipeline::run`] gets only the tail
    pub fn set_chunk_prefix(&mut self, prefix: ChunkPrefix) {
        self.chunk_prefix = (!prefix.is_empty()).then_some(prefix);
    }

    /// Type one recognized chunk while the recording is still running
    ///
    /// The rules apply to the chunk alone: no model sees the phrase, because its answer would
    /// rewrite text that is already in the user's field. A failure leaves the typed prefix in
    /// place and stops further streaming for this phrase.
    pub fn stream_chunk(
        &mut self,
        chunk: &ChunkText,
        app_hint: Option<&str>,
    ) -> Result<StreamProgress, PipelineError> {
        if !self.config.stt.streaming_inject || self.streamed_failed || chunk.text.trim().is_empty()
        {
            return Ok(StreamProgress::default());
        }
        let language = chunk.language.as_deref().unwrap_or("auto");
        let (after_dictionary, hits) = self.dictionary.apply(&chunk.text);
        let text = self
            .rules
            .apply(&after_dictionary, language)
            .trim()
            .to_string();
        if text.is_empty() {
            return Ok(StreamProgress::default());
        }
        self.streamed_dict_hits = self.streamed_dict_hits.saturating_add(hits);
        self.type_streamed(&text, app_hint)?;
        Ok(StreamProgress {
            typed: self.streamed.clone(),
        })
    }

    /// Type one piece of text and remember what the field got
    fn type_streamed(&mut self, text: &str, app_hint: Option<&str>) -> Result<(), PipelineError> {
        let text = text.trim();
        if text.is_empty() || self.streamed_failed {
            return Ok(());
        }
        let separator = if needs_space(&self.streamed, text) {
            " "
        } else {
            ""
        };
        let piece = format!("{separator}{text}");
        let resolved = output_mode(&self.config.output.mode)
            .resolve(&piece, self.config.output.auto_type_max_chars as usize);
        if resolved == OutputMode::Clipboard {
            // A clipboard write replaces the previous one: streaming would leave only the last
            // chunk there. The whole phrase goes to the clipboard at the end instead
            self.streamed_failed = true;
            return Ok(());
        }
        self.injector.set_window(app_hint);
        let started = self.clock.instant();
        match self.injector.inject(&piece, resolved) {
            Ok(_) => {
                self.streamed.push_str(&piece);
                self.streamed_inject_ms = self
                    .streamed_inject_ms
                    .saturating_add(millis_since(started, self.clock.instant()));
                Ok(())
            }
            Err(err) => {
                self.streamed_failed = true;
                Err(PipelineError::Inject(err))
            }
        }
    }

    /// Forget the counters of the typed prefix after a cancelled recording
    ///
    /// The text is already in the user's field: removing it would need a selection and a
    /// backspace, and the cursor may have moved by then. The counters are dropped so that the
    /// next phrase does not inherit them
    pub fn discard_stream(&mut self) {
        self.streamed.clear();
        self.streamed_dict_hits = 0;
        self.streamed_inject_ms = 0;
        self.streamed_failed = false;
    }

    /// Paste the tail after the typed prefix
    fn inject_tail(&mut self, entry: &mut Entry, app_hint: Option<&str>, prefix: &str, tail: &str) {
        let separator = if needs_space(prefix, tail) { " " } else { "" };
        let piece = format!("{separator}{tail}");
        let resolved = output_mode(&self.config.output.mode)
            .resolve(&piece, self.config.output.auto_type_max_chars as usize);
        let target = target_window(app_hint);
        let target = target.as_deref();
        self.injector.set_window(target);
        if let Some(class) = target {
            entry.app = Some(class.to_string());
        }
        let started = self.clock.instant();
        match self.injector.inject(&piece, resolved) {
            Ok(report) => {
                entry.inject_method = Some(inject_method_for(&report.method, target));
                entry.latency_ms.inject = Some(millis_since(started, self.clock.instant()));
            }
            Err(err) => {
                warn!(error = %err, "paste of the tail failed, the typed prefix stays");
                entry.error = Some(err.to_string());
            }
        }
    }

    /// Recognize the whole phrase or, if the start already exists as chunks, only its tail
    fn transcribe_utterance(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
        prefix: &ChunkPrefix,
    ) -> Result<Transcript, SttError> {
        if prefix.is_empty() {
            return transcribe_with_language_policy(self.stt.as_mut(), audio, options);
        }
        let tail = if audio.samples.is_empty() {
            // The last chunk ended exactly at the end of the phrase: nothing to recognize, all is
            // ready
            None
        } else {
            let options = chunked::tail_options(options, prefix);
            Some(transcribe_with_language_policy(
                self.stt.as_mut(),
                audio,
                &options,
            )?)
        };
        Ok(chunked::merge(prefix, tail))
    }

    /// Recognition parameters from the settings and the dictionary
    ///
    /// `timestamps` asks the engine for segments: a phrase never needs them, a file with
    /// timecodes does
    fn stt_options(&self, timestamps: bool) -> SttOptions {
        let initial_prompt = if self.config.dictionary.in_prompt {
            let hint = self.dictionary.prompt_hint();
            (!hint.is_empty()).then_some(hint)
        } else {
            None
        };
        SttOptions {
            language: LanguageHint::parse(&self.config.stt.language),
            allowed_languages: self.config.stt.allowed_languages.clone(),
            initial_prompt,
            threads: self.config.stt.threads as usize,
            timestamps,
        }
    }

    /// Whether this phrase needs the model
    fn wants_llm(&self, style: &crate::domain::text::Style, text: &str) -> bool {
        style.uses_llm
            && self.config.llm.enabled
            && self.config.privacy.send_to_llm
            && self.llm.is_some()
            && word_count(text) >= style.llm_min_words
    }

    /// Dictation: the rules plus, if needed, the model
    fn run_dictation(
        &mut self,
        entry: &mut Entry,
        style: &crate::domain::text::Style,
        after_rules: String,
    ) -> String {
        if !self.wants_llm(style, &after_rules) {
            return after_rules;
        }
        let request = ChatRequest {
            model: self.config.llm.model.clone(),
            system: style
                .prompt_for(entry.language.as_deref().unwrap_or_default())
                .to_string(),
            user: after_rules.clone(),
            temperature: self.config.llm.temperature,
            max_tokens: self.config.llm.max_tokens,
        };
        let attempt = self.complete_with_retries(&request);
        self.record_spend(entry, &attempt.spend);
        match attempt.outcome {
            Ok(outcome) => {
                self.record_llm(entry, &outcome);
                outcome.text
            }
            Err(err) => {
                // The phrase is not lost: the text after the rules is returned. The failure
                // belongs in the journal too, otherwise the window shows a successful phrase
                // and the owner cannot tell that the model was never used
                warn!(error = %err, "post-processing failed, returning the text after the rules");
                entry.error = Some(format!("LLM: {err}"));
                after_rules
            }
        }
    }

    /// Command mode: the instruction is applied to the selected text
    fn run_command(
        &mut self,
        entry: &mut Entry,
        selection: &str,
        instruction: &str,
    ) -> Result<String, PipelineError> {
        if self.llm.is_none() || !self.config.llm.enabled {
            return Err(PipelineError::CommandModeNeedsLlm(LlmError::Disabled));
        }
        if !self.config.privacy.send_to_llm {
            return Err(PipelineError::CommandModeNeedsLlm(LlmError::Disabled));
        }
        let lang = entry.language.as_deref().unwrap_or_default();
        let request = ChatRequest {
            model: self.config.llm.model.clone(),
            system: command_mode::system_prompt(&self.config.command_mode.system_prompt, lang),
            user: command_mode::request(lang, instruction, selection),
            temperature: self.config.llm.temperature,
            max_tokens: self.config.llm.max_tokens,
        };
        let attempt = self.complete_with_retries(&request);
        self.record_spend(entry, &attempt.spend);
        match attempt.outcome {
            Ok(outcome) => {
                self.record_llm(entry, &outcome);
                Ok(outcome.text)
            }
            Err(err) => Err(PipelineError::CommandModeNeedsLlm(err)),
        }
    }

    /// Request to the model with a limited number of retries
    ///
    /// An authorization error is not retried: a retry does not fix the key
    fn complete_with_retries(&self, request: &ChatRequest) -> LlmAttempt {
        let Some(llm) = self.llm.as_ref() else {
            return LlmAttempt {
                outcome: Err(LlmError::Disabled),
                spend: LlmSpend::default(),
            };
        };
        let attempts = self.config.llm.max_retries.saturating_add(1);
        let started = self.clock.instant();
        let mut spend = LlmSpend::default();
        let mut last = LlmError::Disabled;
        for attempt in 1..=attempts {
            match llm.complete(request) {
                Ok(response) => {
                    spend.add(response.cost_usd);
                    let tokens = match (response.prompt_tokens, response.completion_tokens) {
                        (None, None) => None,
                        (prompt, completion) => Some(Tokens {
                            prompt: prompt.unwrap_or(0),
                            completion: completion.unwrap_or(0),
                        }),
                    };
                    // Service tags, fences, introductory phrases and the dictionary echo must not
                    // reach the user's field (AM-19)
                    let text = sanitize_llm_output(&response.text, &self.dictionary.prompt_hint());
                    if text.is_empty() {
                        // The answer was paid for and thrown away: the next attempt pays again,
                        // and the journal holds both charges. The provider's own reason travels
                        // in the note, so the last attempt can carry it into the error
                        last = LlmError::BadResponse(
                            response
                                .note
                                .unwrap_or_else(|| "the model returned empty text".into()),
                        );
                        continue;
                    }
                    return LlmAttempt {
                        outcome: Ok(LlmOutcome {
                            text,
                            tokens,
                            elapsed_ms: millis_since(started, self.clock.instant()),
                        }),
                        spend,
                    };
                }
                // A rejected key is not a charge, and a retry does not fix the key
                Err(LlmError::Auth) => {
                    return LlmAttempt {
                        outcome: Err(LlmError::Auth),
                        spend,
                    };
                }
                Err(err) => {
                    warn!(attempt, attempts, error = %err, "retry of the request to the model");
                    // The provider may have read the request and dropped the answer, or never
                    // seen it: the money is unknown, not zero
                    spend.calls += 1;
                    spend.unpriced += 1;
                    last = err;
                }
            }
        }
        LlmAttempt {
            outcome: Err(last),
            spend,
        }
    }

    /// Mark in the entry that the model ran
    fn record_llm(&self, entry: &mut Entry, outcome: &LlmOutcome) {
        entry.llm_used = true;
        entry.llm_provider = self.llm.as_ref().map(|llm| llm.id().to_string());
        entry.llm_model = Some(self.config.llm.model.clone());
        entry.local_llm = self.llm.as_ref().is_some_and(|llm| llm.is_local());
        entry.tokens.clone_from(&outcome.tokens);
        entry.latency_ms.llm = Some(outcome.elapsed_ms);
    }

    /// Give the entry the money the model calls spent, including the failed ones
    fn record_spend(&self, entry: &mut Entry, spend: &LlmSpend) {
        let local = self.llm.as_ref().is_some_and(|llm| llm.is_local());
        entry.llm_cost_usd = spend.cost_usd;
        entry.llm_calls = spend.calls;
        // A local model costs nothing, and that is a known zero: it has no unpriced call, even
        // when it reports no price at all
        entry.llm_unpriced_calls = if local { 0 } else { spend.unpriced };
        entry.local_llm = local;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::app::dictionary::Term;
    use crate::config::Snippet;
    use crate::domain::fakes::{FakeClock, FakeLlm, FakeStt, MemJournal, RecordingInjector};
    use crate::domain::inject::{InjectError, InjectReport};
    use crate::domain::journal::Journal as JournalTrait;
    use crate::domain::llm::ChatResponse;
    use crate::domain::stt::Transcript;

    /// A fake that the test can still reach after it is passed to the pipeline
    #[derive(Debug)]
    struct SharedInjector(Arc<Mutex<RecordingInjector>>);

    impl TextInjector for SharedInjector {
        fn id(&self) -> &'static str {
            "shared"
        }

        fn available(&self) -> bool {
            true
        }

        fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
            self.0.lock().unwrap().inject(text, mode)
        }

        fn copy_selection(&mut self) -> Result<String, InjectError> {
            self.0.lock().unwrap().copy_selection()
        }
    }

    #[derive(Debug)]
    struct SharedJournal(Arc<Mutex<MemJournal>>);

    impl JournalTrait for SharedJournal {
        fn append(&mut self, entry: &Entry) -> Result<(), JournalError> {
            self.0.lock().unwrap().append(entry)
        }
    }

    #[derive(Debug)]
    struct SharedStt(Arc<Mutex<FakeStt>>);

    impl SttEngine for SharedStt {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn model_name(&self) -> &'static str {
            "fake"
        }

        fn transcribe(
            &mut self,
            audio: &PcmAudio,
            opts: &SttOptions,
        ) -> Result<Transcript, SttError> {
            self.0.lock().unwrap().transcribe(audio, opts)
        }

        fn unload(&mut self) {
            self.0.lock().unwrap().unload();
        }
    }

    struct Harness {
        pipeline: Pipeline,
        injector: Arc<Mutex<RecordingInjector>>,
        journal: Arc<Mutex<MemJournal>>,
        stt: Arc<Mutex<FakeStt>>,
        clock: Arc<FakeClock>,
    }

    fn audio(secs: f32) -> PcmAudio {
        PcmAudio::new(vec![0.1; (secs * 16_000.0) as usize], 16_000)
    }

    fn clock() -> Arc<FakeClock> {
        Arc::new(FakeClock::at(
            chrono::DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ))
    }

    fn build(transcript: &str, llm: Option<Arc<FakeLlm>>, mut config: PipelineConfig) -> Harness {
        // These tests exercise the streaming machinery explicitly; the shipped default is off
        config.stt.streaming_inject = true;
        let stt = Arc::new(Mutex::new(FakeStt::returning(transcript)));
        let injector = Arc::new(Mutex::new(RecordingInjector::default()));
        let journal = Arc::new(Mutex::new(MemJournal::default()));
        let clock = clock();
        let pipeline = Pipeline::new(
            Box::new(SharedStt(Arc::clone(&stt))),
            llm.map(|llm| llm as Arc<dyn LlmClient>),
            Box::new(SharedInjector(Arc::clone(&injector))),
            Box::new(SharedJournal(Arc::clone(&journal))),
            clock.clone(),
            config,
        );
        Harness {
            pipeline,
            injector,
            journal,
            stt,
            clock,
        }
    }

    /// Settings with the model on: by default `llm.enabled = false`
    fn with_llm() -> PipelineConfig {
        let mut config = PipelineConfig::default();
        config.llm.enabled = true;
        config
    }

    /// A custom style with the same id takes the built-in place, so the word minimum is the one
    /// the test names
    fn set_min_words(config: &mut PipelineConfig, words: u32) {
        config.style.custom = vec![crate::config::CustomStyle {
            id: "cleanup".into(),
            name: "Cleanup".into(),
            uses_llm: true,
            system_prompt: "Fix the text.".into(),
            llm_min_words: words,
        }];
    }

    fn injected(harness: &Harness) -> Vec<String> {
        harness
            .injector
            .lock()
            .unwrap()
            .injected
            .iter()
            .map(|(text, _)| text.clone())
            .collect()
    }

    #[test]
    fn the_injector_receives_the_text_after_the_dictionary_and_the_rules() {
        let mut harness = build(
            "ну молва запятая это работает точка",
            None,
            PipelineConfig::default(),
        );
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, Some("kitty"))
            .unwrap();

        assert_eq!(injected(&harness), vec!["MolvAI, это работает."]);
        assert_eq!(entry.text_final.as_deref(), Some("MolvAI, это работает."));
        assert_eq!(
            entry.text_raw.as_deref(),
            Some("ну молва запятая это работает точка")
        );
        assert_eq!(entry.dict_hits, 1);
        assert_eq!(entry.app.as_deref(), Some("kitty"));
        assert!(!entry.llm_used);
    }

    #[test]
    fn a_failing_model_still_delivers_the_text_after_the_rules() {
        let llm = Arc::new(FakeLlm::failing(LlmError::Timeout(20)));
        let text = "это достаточно длинная реплика из более чем десяти слов запятая \
                    чтобы модель точно вызвалась точка";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        assert!(llm.calls() >= 1, "the model must have been called");
        assert!(!entry.llm_used);
        assert!(
            entry
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("LLM:")),
            "the journal must say that the model failed: {:?}",
            entry.error
        );
        assert_eq!(injected(&harness).len(), 1);
        assert!(
            injected(&harness)[0].ends_with('.'),
            "{:?}",
            injected(&harness)
        );
        assert!(entry.tokens.is_none());
        assert!(entry.latency_ms.llm.is_none());
    }

    #[test]
    fn an_utterance_below_the_style_minimum_never_reaches_the_model() {
        let mut config = with_llm();
        set_min_words(&mut config, 10);
        let llm = Arc::new(FakeLlm::echoing("переписано моделью"));
        let mut harness = build("привет мир как дела", Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(3.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
        assert!(!entry.llm_used);
        assert_eq!(injected(&harness), vec!["Привет мир как дела"]);
    }

    #[test]
    fn a_phrase_at_the_style_minimum_reaches_the_model() {
        let mut config = with_llm();
        set_min_words(&mut config, 3);
        let llm = Arc::new(FakeLlm::echoing("переписано моделью"));
        let mut harness = build("привет мир как", Some(Arc::clone(&llm)), config);
        harness
            .pipeline
            .run(audio(3.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 1);
    }

    #[test]
    fn a_long_utterance_goes_through_the_model_and_records_tokens() {
        let llm = Arc::new(FakeLlm::echoing("Переписанный моделью текст."));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 1);
        assert!(entry.llm_used);
        assert_eq!(entry.llm_provider.as_deref(), Some("fake"));
        assert_eq!(entry.llm_model.as_deref(), Some("qwen3.5:4b"));
        assert!(entry.local_llm);
        assert_eq!(
            entry.tokens,
            Some(Tokens {
                prompt: 10,
                completion: 5
            })
        );
        assert!(entry.latency_ms.llm.is_some());
        assert_eq!(injected(&harness), vec!["Переписанный моделью текст."]);
    }

    #[test]
    fn the_charge_of_the_provider_reaches_the_journal_entry() {
        let llm = Arc::new(FakeLlm::new(|_| {
            Ok(ChatResponse {
                text: "Переписанный моделью текст.".into(),
                prompt_tokens: Some(10),
                completion_tokens: Some(5),
                cost_usd: Some(0.000_004),
                note: None,
            })
        }));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(entry.llm_cost_usd, Some(0.000_004));
    }

    #[test]
    fn a_phrase_that_never_reaches_the_model_records_no_charge() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let mut harness = build("привет", Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(1.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 0);
        assert_eq!(entry.llm_cost_usd, None);
    }

    #[test]
    fn a_retry_that_was_billed_enters_the_charge_of_the_phrase() {
        // The first answer comes back empty: paid for, thrown away, and the second attempt is
        // paid as well
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let llm = Arc::new(FakeLlm::new(move |_| {
            let call = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ChatResponse {
                text: if call == 0 {
                    String::new()
                } else {
                    "Переписанный моделью текст.".into()
                },
                prompt_tokens: Some(10),
                completion_tokens: Some(5),
                cost_usd: Some(0.000_004),
                note: Some("the answer has no text".into()),
            })
        }));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 2);
        assert_eq!(entry.llm_calls, 2);
        assert_eq!(entry.llm_unpriced_calls, 0);
        assert_eq!(entry.llm_cost_usd, Some(0.000_008));
    }

    #[test]
    fn a_phrase_whose_calls_all_failed_keeps_what_they_cost() {
        let llm = Arc::new(FakeLlm::new(|_| {
            Ok(ChatResponse {
                text: String::new(),
                prompt_tokens: Some(10),
                completion_tokens: Some(0),
                cost_usd: Some(0.000_003),
                note: Some("provider error: upstream provider failed".into()),
            })
        }));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        // The text after the rules reaches the field, and the money is not lost with the answer
        assert!(!entry.llm_used);
        assert_eq!(entry.llm_calls, 3);
        assert_eq!(entry.llm_cost_usd, Some(0.000_009));
        let error = entry.error.unwrap_or_default();
        assert!(error.contains("upstream provider failed"), "{error}");
    }

    #[test]
    fn the_verbatim_style_never_reaches_the_model() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, Some("verbatim"), None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
        assert_eq!(entry.style, "verbatim");
    }

    #[test]
    fn privacy_forbids_sending_anything_to_the_model() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let mut config = with_llm();
        config.privacy.send_to_llm = false;
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
        assert!(!entry.llm_used);
    }

    #[test]
    fn a_disabled_model_leaves_the_rules_in_charge() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        // llm.enabled = false in the default settings
        let mut harness = build(text, Some(Arc::clone(&llm)), PipelineConfig::default());
        harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
    }

    #[test]
    fn retries_are_bounded_by_the_configuration() {
        let llm = Arc::new(FakeLlm::failing(LlmError::Unavailable("down".into())));
        let mut config = with_llm();
        config.llm.max_retries = 2;
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), config);
        harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 3, "one attempt plus two retries");
    }

    #[test]
    fn a_rejected_key_is_not_retried() {
        let llm = Arc::new(FakeLlm::failing(LlmError::Auth));
        let mut config = with_llm();
        config.llm.max_retries = 3;
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 1, "a retry does not fix the key");
        assert!(!entry.llm_used);
    }

    #[test]
    fn the_journal_gets_one_entry_with_all_the_metrics() {
        let mut harness = build("привет мир", None, PipelineConfig::default());
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, Some("kitty"))
            .unwrap();
        let stored = harness.journal.lock().unwrap().entries.clone();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, entry.id);
        assert_eq!(stored[0].session_id, harness.pipeline.session_id());
        assert_eq!(stored[0].words, 2);
        assert_eq!(stored[0].wpm, Entry::wpm_for(2, 4.0));
        assert!((stored[0].audio_secs - 4.0).abs() < 1e-3);
        assert_eq!(stored[0].stt_engine, "fake");
        assert!(stored[0].inject_method.is_some());
    }

    #[test]
    fn no_record_mode_keeps_the_journal_empty_but_still_injects() {
        let mut config = PipelineConfig::default();
        config.privacy.no_record_mode = true;
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(harness.journal.lock().unwrap().entries.is_empty());
        assert_eq!(injected(&harness), vec!["Привет мир"]);
    }

    #[test]
    fn a_failed_injection_lands_in_the_entry_and_the_journal() {
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness.injector.lock().unwrap().fail_with =
            Some(InjectError::Failed("no active window".into()));
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(entry.error.is_some(), "a paste error must reach the entry");
        assert!(entry.error.unwrap().contains("no active window"));
        assert_eq!(entry.text_final.as_deref(), Some("Привет мир"));
        assert_eq!(harness.journal.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn an_empty_transcript_is_not_injected_but_is_recorded() {
        let mut harness = build("   ", None, PipelineConfig::default());
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(injected(&harness).is_empty());
        assert_eq!(entry.words, 0);
        assert!(entry.error.is_some());
        assert_eq!(harness.journal.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn stt_options_carry_the_language_and_the_dictionary_hint() {
        let mut config = PipelineConfig::default();
        config.stt.language = "ru".into();
        let mut harness = build("привет", None, config);
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        harness
            .pipeline
            .run(audio(2.0), Mode::Dictation, None, None)
            .unwrap();
        let calls = harness.stt.lock().unwrap().calls.clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].language, LanguageHint::Fixed("ru".into()));
        assert_eq!(calls[0].initial_prompt.as_deref(), Some("MolvAI"));
        assert_eq!(calls[0].allowed_languages, vec!["ru", "en"]);
    }

    #[test]
    fn the_dictionary_hint_can_be_switched_off() {
        let mut config = PipelineConfig::default();
        config.dictionary.in_prompt = false;
        let mut harness = build("привет", None, config);
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        harness
            .pipeline
            .run(audio(2.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(harness.stt.lock().unwrap().calls[0].initial_prompt, None);
    }

    #[test]
    fn the_window_class_picks_the_style_when_nothing_is_requested() {
        let mut config = with_llm();
        config
            .style
            .by_app
            .insert("kitty".into(), "verbatim".into());
        let mut harness = build("привет мир", None, config);
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, Some("kitty"))
            .unwrap();
        assert_eq!(entry.style, "verbatim");
    }

    #[test]
    fn a_snippet_returns_its_replacement_instead_of_the_phrase() {
        let phrase = "ответь клиенту очень подробно и вежливо прямо сейчас";
        let mut config = with_llm();
        config.snippets.items = vec![Snippet {
            phrase: phrase.into(),
            replacement: "Добрый день! Уже подготовил ответ.".into(),
        }];
        let llm = Arc::new(FakeLlm::echoing("этот ответ не должен понадобиться"));
        let mut harness = build(phrase, Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(
            injected(&harness),
            vec!["Добрый день! Уже подготовил ответ."]
        );
        assert_eq!(llm.calls(), 0);
        assert!(!entry.llm_used);
        assert!(entry.snippet);
        assert_eq!(
            entry.text_final.as_deref(),
            Some("Добрый день! Уже подготовил ответ.")
        );
    }

    #[test]
    fn a_wildcard_snippet_returns_its_replacement_without_the_model() {
        let phrase = "ответь клиенту очень подробно и вежливо прямо сейчас";
        let mut config = with_llm();
        config.snippets.items = vec![Snippet {
            phrase: "ответь *".into(),
            replacement: "Добрый день!".into(),
        }];
        let llm = Arc::new(FakeLlm::echoing("этот ответ не должен понадобиться"));
        let mut harness = build(phrase, Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(injected(&harness), vec!["Добрый день!"]);
        assert_eq!(llm.calls(), 0);
        assert!(!entry.llm_used);
        assert!(entry.snippet);
    }

    #[test]
    fn a_switched_off_snippet_list_leaves_the_phrase_alone() {
        let phrase = "ответь клиенту очень подробно и вежливо прямо сейчас";
        let mut config = PipelineConfig::default();
        config.snippets.items = vec![Snippet {
            phrase: phrase.into(),
            replacement: "Добрый день!".into(),
        }];
        config.snippets.enabled = false;
        let mut harness = build(phrase, None, config);
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(injected(&harness), vec![entry.text_final.clone().unwrap()]);
        assert!(!entry.llm_used);
        assert!(!entry.snippet);
        assert_ne!(entry.text_final.as_deref(), Some("Добрый день!"));
    }

    #[test]
    fn a_snippet_does_not_match_a_longer_phrase() {
        let mut config = with_llm();
        set_min_words(&mut config, 0);
        config.snippets.items = vec![Snippet {
            phrase: "ответь клиенту".into(),
            replacement: "Добрый день!".into(),
        }];
        let llm = Arc::new(FakeLlm::echoing("ответ модели"));
        let mut harness = build(
            "ответь клиенту очень подробно и вежливо прямо сейчас",
            Some(Arc::clone(&llm)),
            config,
        );
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 1);
        assert_eq!(injected(&harness), vec!["ответ модели"]);
        assert!(entry.llm_used);
    }

    #[test]
    fn command_mode_applies_the_instruction_to_the_selection() {
        let llm = Arc::new(FakeLlm::echoing("Исправленный выделенный текст."));
        let mut config = with_llm();
        config.stt.language = "ru".into();
        let mut harness = build("сделай текст официальным", Some(Arc::clone(&llm)), config);
        harness.injector.lock().unwrap().selection = Some("привет как сам".into());

        let entry = harness
            .pipeline
            .run(audio(3.0), Mode::Command, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 1);
        let request = llm.last_request.lock().unwrap().clone().unwrap();
        assert!(request.user.contains("привет как сам"), "{}", request.user);
        assert!(
            request.user.contains("сделай текст официальным"),
            "{}",
            request.user
        );
        // The default prompt tells the model to return only text: no headings, no explanations
        assert!(
            request.system.contains("только получившийся текст"),
            "{}",
            request.system
        );
        assert_eq!(injected(&harness), vec!["Исправленный выделенный текст."]);
        assert_eq!(entry.mode, Mode::Command);
        assert!(entry.llm_used);
    }

    fn has_cyrillic(text: &str) -> bool {
        text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
    }

    /// What the model receives for one phrase in `lang`, in `mode`
    fn request_for(lang: &str, mode: Mode, transcript: &str) -> ChatRequest {
        let llm = Arc::new(FakeLlm::echoing("Result."));
        let mut config = with_llm();
        config.stt.language = lang.into();
        let mut harness = build(transcript, Some(Arc::clone(&llm)), config);
        harness.injector.lock().unwrap().selection = Some("selected text".into());
        harness.pipeline.run(audio(8.0), mode, None, None).unwrap();
        let request = llm.last_request.lock().unwrap().clone();
        request.expect("the model must have been called")
    }

    const LONG_PHRASE: &str = "one two three four five six seven eight nine ten eleven twelve";

    #[test]
    fn the_style_prompt_follows_the_language_of_the_phrase() {
        let ru = request_for("ru", Mode::Dictation, LONG_PHRASE);
        assert!(has_cyrillic(&ru.system), "{}", ru.system);
        let en = request_for("en", Mode::Dictation, LONG_PHRASE);
        assert!(!has_cyrillic(&en.system), "{}", en.system);
        assert!(en.system.contains("only the result"), "{}", en.system);
    }

    #[test]
    fn command_mode_speaks_the_language_of_the_instruction() {
        let ru = request_for("ru", Mode::Command, "сделай короче");
        assert!(has_cyrillic(&ru.system), "{}", ru.system);
        assert!(ru.user.starts_with("ИНСТРУКЦИЯ"), "{}", ru.user);
        let en = request_for("en", Mode::Command, "make it shorter");
        assert!(!has_cyrillic(&en.system), "{}", en.system);
        assert!(!has_cyrillic(&en.user), "{}", en.user);
        assert!(en.user.contains("selected text"), "{}", en.user);
    }

    #[test]
    fn a_command_prompt_from_the_settings_is_sent_in_every_language() {
        for lang in ["ru", "en"] {
            let llm = Arc::new(FakeLlm::echoing("Result."));
            let mut config = with_llm();
            config.stt.language = lang.into();
            config.command_mode.system_prompt = "Answer in verse.".into();
            let mut harness = build("instruction", Some(Arc::clone(&llm)), config);
            harness.injector.lock().unwrap().selection = Some("text".into());
            harness
                .pipeline
                .run(audio(3.0), Mode::Command, None, None)
                .unwrap();
            let request = llm.last_request.lock().unwrap().clone().unwrap();
            assert_eq!(request.system, "Answer in verse.", "{lang}");
        }
    }

    #[test]
    fn command_mode_is_short_even_though_the_model_is_still_called() {
        // The llm_min_words threshold does not apply to voice editing: without the model it is
        // meaningless
        let llm = Arc::new(FakeLlm::echoing("Готово."));
        let mut harness = build("покороче", Some(Arc::clone(&llm)), with_llm());
        harness.injector.lock().unwrap().selection = Some("длинный выделенный текст".into());
        harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 1);
    }

    #[test]
    fn command_mode_without_a_model_is_an_explicit_error() {
        let mut harness = build("сделай короче", None, PipelineConfig::default());
        harness.injector.lock().unwrap().selection = Some("текст".into());
        let err = harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap_err();
        assert!(
            matches!(err, PipelineError::CommandModeNeedsLlm(_)),
            "{err:?}"
        );
    }

    #[test]
    fn command_mode_without_a_selection_is_an_explicit_error() {
        let llm = Arc::new(FakeLlm::echoing("ок"));
        let mut harness = build("сделай короче", Some(llm), with_llm());
        harness.injector.lock().unwrap().selection = None;
        let err = harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap_err();
        assert!(matches!(err, PipelineError::Selection(_)), "{err:?}");
    }

    #[test]
    fn command_mode_can_be_switched_off() {
        let llm = Arc::new(FakeLlm::echoing("ок"));
        let mut config = with_llm();
        config.command_mode.enabled = false;
        let mut harness = build("сделай короче", Some(llm), config);
        let err = harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap_err();
        assert!(matches!(err, PipelineError::CommandModeDisabled), "{err:?}");
    }

    #[test]
    fn a_recognition_failure_is_an_error_and_nothing_is_injected() {
        let stt = Arc::new(Mutex::new(FakeStt::with_responses(vec![Err(
            SttError::ModelLoad("no model file".into()),
        )])));
        let injector = Arc::new(Mutex::new(RecordingInjector::default()));
        let journal = Arc::new(Mutex::new(MemJournal::default()));
        let mut pipeline = Pipeline::new(
            Box::new(SharedStt(Arc::clone(&stt))),
            None,
            Box::new(SharedInjector(Arc::clone(&injector))),
            Box::new(SharedJournal(Arc::clone(&journal))),
            clock(),
            PipelineConfig::default(),
        );
        let err = pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap_err();
        assert!(matches!(err, PipelineError::Stt(_)), "{err:?}");
        assert!(injector.lock().unwrap().injected.is_empty());
        assert!(journal.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn the_output_mode_from_the_configuration_reaches_the_injector() {
        let mut config = PipelineConfig::default();
        config.output.mode = "clipboard".into();
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        let modes: Vec<OutputMode> = harness
            .injector
            .lock()
            .unwrap()
            .injected
            .iter()
            .map(|(_, mode)| *mode)
            .collect();
        assert_eq!(modes, vec![OutputMode::Clipboard]);
    }

    #[test]
    fn auto_output_mode_is_resolved_before_the_injector_sees_it() {
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        let modes: Vec<OutputMode> = harness
            .injector
            .lock()
            .unwrap()
            .injected
            .iter()
            .map(|(_, mode)| *mode)
            .collect();
        assert_eq!(modes, vec![OutputMode::Type], "a short text is typed");
    }

    #[test]
    fn service_tags_from_the_model_never_reach_the_injector() {
        // Criterion AM-19: reasoning and fences stay with the model, and the field gets text
        let llm = Arc::new(FakeLlm::echoing(
            "<think>подумаю ещё</think>\n```\nСобрание переносится на среду.\n```",
        ));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(llm), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(injected(&harness), vec!["Собрание переносится на среду."]);
        assert_eq!(
            entry.text_final.as_deref(),
            Some("Собрание переносится на среду.")
        );
    }

    #[test]
    fn command_mode_output_is_cleaned_up_too() {
        let llm = Arc::new(FakeLlm::echoing(
            "Вот исправленный текст: \"Добрый день, коллеги.\"",
        ));
        let mut harness = build("сделай официальным", Some(llm), with_llm());
        harness.injector.lock().unwrap().selection = Some("привет как сам".into());
        harness
            .pipeline
            .run(audio(3.0), Mode::Command, None, None)
            .unwrap();
        assert_eq!(injected(&harness), vec!["Добрый день, коллеги."]);
    }

    #[test]
    fn a_model_that_echoes_the_dictionary_is_treated_as_a_failure() {
        // A reply that is only the dictionary hint is not a phrase: the text from the rules stays
        let llm = Arc::new(FakeLlm::echoing("MolvAI"));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(!entry.llm_used, "the dictionary echo is not a model reply");
        assert!(
            injected(&harness)[0].starts_with("Это достаточно длинная"),
            "{:?}",
            injected(&harness)
        );
    }

    #[test]
    fn the_pause_before_the_injection_comes_from_the_configuration() {
        // Criterion AM-20: the pause before the paste is set by a setting, and is not hard-coded
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(
            harness.clock.slept(),
            vec![Duration::from_millis(50)],
            "по умолчанию перед вставкой ждём 50 мс"
        );

        // A remote desktop needs more time to return the focus
        let mut config = PipelineConfig::default();
        config.output.pre_inject_delay_ms = 1500;
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(harness.clock.slept(), vec![Duration::from_millis(1500)]);
    }

    #[test]
    fn the_microphone_release_measurement_lands_in_the_entry_once() {
        // The microphone privacy guarantee must be visible in the journal. The daemon measurement
        // goes into the entry of the phrase it belongs to, and does not move to the next one
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness.pipeline.set_stop_after_release(120);
        let first = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(first.latency_ms.stop_after_release, Some(120));
        let second = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(second.latency_ms.stop_after_release, None);
    }

    #[test]
    fn a_zero_pause_does_not_wait_at_all() {
        let mut config = PipelineConfig::default();
        config.output.pre_inject_delay_ms = 0;
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(harness.clock.slept().is_empty());
    }

    #[test]
    fn nothing_to_inject_means_nothing_to_wait_for() {
        // An empty phrase is not pasted, and there must be no pause before the paste either
        let mut harness = build("   ", None, PipelineConfig::default());
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(harness.clock.slept().is_empty());
    }

    #[test]
    fn without_an_active_window_the_journal_says_the_text_stayed_in_the_clipboard() {
        // Criterion AJ-09/AJ-10: there is no input field, so the text is in the clipboard, and
        // the history shows it
        assert_eq!(inject_method_for("clipboard-only", None), NO_FOCUS_METHOD);
        assert_eq!(
            inject_method_for("clipboard-only", Some("kitty")),
            "clipboard-only",
            "a window exists: an ordinary paste through the clipboard, even if with Ctrl+V by hand"
        );
        assert_eq!(inject_method_for("wtype-type", None), "wtype-type");
    }

    #[test]
    fn streaming_types_the_chunks_and_the_tail_without_losing_words() {
        let mut harness = build("утром", None, PipelineConfig::default());
        for text in ["мама", "мыла", "раму"] {
            harness
                .pipeline
                .stream_chunk(
                    &ChunkText {
                        text: text.into(),
                        language: Some("ru".into()),
                        stt_ms: 0,
                    },
                    None,
                )
                .expect("the chunk is typed");
        }
        harness.pipeline.set_chunk_prefix(ChunkPrefix {
            text: "мама мыла раму".into(),
            language: Some("ru".into()),
            audio_secs: Some(20.0),
            ..ChunkPrefix::default()
        });

        let entry = harness
            .pipeline
            .run(audio(2.0), Mode::Dictation, None, None)
            .expect("the phrase is processed");

        let final_text = entry.text_final.clone().unwrap_or_default();
        assert!(
            final_text.contains("Утром"),
            "the tail was lost: {final_text:?}"
        );
        let typed = injected(&harness).join("");
        assert_eq!(
            typed.trim(),
            final_text.trim(),
            "the field and the journal disagree"
        );
        assert!(
            entry
                .text_raw
                .as_deref()
                .unwrap_or_default()
                .contains("утром"),
            "the raw text lost the tail"
        );
    }

    #[test]
    fn output_mode_parsing_falls_back_to_auto() {
        assert_eq!(output_mode("paste"), OutputMode::Paste);
        assert_eq!(output_mode(" TYPE "), OutputMode::Type);
        assert_eq!(output_mode("clipboard"), OutputMode::Clipboard);
        assert_eq!(output_mode("something else"), OutputMode::Auto);
    }
    #[test]
    fn a_cancelled_file_stops_before_the_model_reads_it() {
        let mut config = PipelineConfig::default();
        config.stt.chunk_pause_ms = 300;
        let mut harness = build("фраза", None, config);
        // The client gave up before the job started: the flag is already set
        let cancel = Arc::new(crate::domain::cancel::CancelToken::new());
        cancel.cancel();
        harness.pipeline.set_file_cancel(cancel);
        let request = FileTranscriptionRequest {
            path: std::path::PathBuf::from("/tmp/lecture.wav"),
            style: None,
            timecodes: true,
            no_llm: true,
            source: Source::File,
            chunked: true,
        };
        let error = harness
            .pipeline
            .transcribe_audio(
                String::from("lecture.wav"),
                PcmAudio::new(vec![0.1; 16_000 * 4], 16_000),
                &request,
            )
            .expect_err("the file was cancelled");
        assert!(error.contains("cancelled"), "{error}");
        assert!(
            harness.stt.lock().unwrap().calls.is_empty(),
            "the model was asked after the cancel"
        );
    }

    #[test]
    fn a_chunked_file_is_recognized_at_its_pauses() {
        let mut config = PipelineConfig::default();
        config.stt.chunk_pause_ms = 300;
        config.audio.silence_threshold_db = -45.0;
        let mut harness = build("фраза", None, config);
        let mut samples = vec![0.1; 3_200 * 16];
        samples.extend(vec![0.0; 800 * 16]);
        samples.extend(vec![0.1; 800 * 16]);
        let request = FileTranscriptionRequest {
            path: std::path::PathBuf::from("/tmp/lecture.wav"),
            style: None,
            timecodes: true,
            no_llm: true,
            source: Source::File,
            chunked: true,
        };
        let result = harness
            .pipeline
            .transcribe_audio(
                "lecture.wav".to_string(),
                PcmAudio::new(samples, 16_000),
                &request,
            )
            .expect("the file is transcribed");
        assert_eq!(result.text, "Фраза");
        assert!(
            harness.stt.lock().unwrap().calls.len() >= 2,
            "the file was not cut at its pause"
        );
        // One segment per chunk: its bounds are the pauses the file was cut at, not the inner
        // split of the model, which is finer than the audio of one chunk
        assert_eq!(result.segments.len(), 2, "{:?}", result.segments);
        assert_eq!(result.segments[0].start_ms, 0);
        assert!(
            result.segments[0].end_ms >= 3_000 && result.segments[0].end_ms <= 4_000,
            "the first chunk ends at its pause: {:?}",
            result.segments[0]
        );
        assert!(result.segments[1].start_ms >= result.segments[0].end_ms);
        assert!(result.segments[1].end_ms > result.segments[1].start_ms);
    }
}
