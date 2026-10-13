// SPDX-License-Identifier: MIT
//! Processing of recorded audio: recognize → paste → write to the journal
//!
//! `Processor` is the seam between the daemon and the pipeline. The daemon knows only this
//! trait, so the full pipeline (rules, dictionary, model) plugs in later without changes to the
//! daemon

use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;
use uuid::Uuid;

use crate::config::{Config, OutputConfig};
use crate::domain::audio::PcmAudio;
use crate::domain::clock::Clock;
use crate::domain::entry::{Entry, LatencyMs, Mode, Source, SCHEMA_VERSION};
use crate::domain::inject::{InjectError, TextInjector};
use crate::domain::journal::{Journal, JournalError};
use crate::domain::notify::Notifier;
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions};
use crate::domain::text::word_count;
use crate::infra::inject::parse_output_mode;

use super::chunked::{ChunkContext, ChunkPrefix, ChunkText};

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("recognition failed: {0}")]
    Stt(#[from] SttError),
    #[error("paste failed: {0}")]
    Inject(#[from] InjectError),
    #[error("cannot write to the journal: {0}")]
    Journal(#[from] JournalError),
    #[error("{0}")]
    Pipeline(String),
}

impl From<crate::app::pipeline::PipelineError> for ProcessError {
    fn from(err: crate::app::pipeline::PipelineError) -> Self {
        use crate::app::pipeline::PipelineError as P;
        match err {
            P::Stt(inner) => ProcessError::Stt(inner),
            P::Journal(inner) => ProcessError::Journal(inner),
            other => ProcessError::Pipeline(other.to_string()),
        }
    }
}

/// The full pipeline (dictionary, rules, model) in the place of the daemon processor
impl Processor for crate::app::pipeline::Pipeline {
    fn apply_config(&mut self, config: &Config) {
        self.set_config(crate::app::pipeline::PipelineConfig::from_config(config));
    }

    fn process(
        &mut self,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, ProcessError> {
        Ok(self.run(audio, mode, style, app_hint)?)
    }

    fn set_stop_after_release(&mut self, ms: u32) {
        crate::app::pipeline::Pipeline::set_stop_after_release(self, ms);
    }

    fn transcribe_chunk(
        &mut self,
        audio: &PcmAudio,
        context: &ChunkContext,
    ) -> Option<Result<ChunkText, ProcessError>> {
        Some(
            crate::app::pipeline::Pipeline::transcribe_chunk(self, audio, context)
                .map_err(ProcessError::from),
        )
    }

    fn process_with_prefix(
        &mut self,
        prefix: ChunkPrefix,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, ProcessError> {
        self.set_chunk_prefix(prefix);
        self.process(audio, mode, style, app_hint)
    }
}

/// Pipeline that processes one phrase
///
/// The implementation must return an `Entry` even when the paste failed. A paste failure is the
/// `error` field in the entry, not a failure of the whole processing, or the user loses the
/// recognized text
pub trait Processor: std::fmt::Debug + Send {
    fn process(
        &mut self,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, ProcessError>;

    /// Time from the key release to the close of the microphone stream, in milliseconds
    ///
    /// The daemon measures it, because only the daemon knows both points. The value goes into
    /// `Entry.latency_ms.stop_after_release`, so the journal shows the microphone privacy
    /// guarantee
    fn set_stop_after_release(&mut self, _ms: u32) {}

    /// Apply the reloaded settings: the default style, the rules, the thresholds. A processor
    /// that does not care about the settings does nothing
    fn apply_config(&mut self, _config: &Config) {}

    /// Recognize a chunk of a recording that is still running
    ///
    /// `None` means that the processor cannot do streaming processing: the daemon then waits
    /// for the end of the phrase and gives it whole, as before
    fn transcribe_chunk(
        &mut self,
        _audio: &PcmAudio,
        _context: &ChunkContext,
    ) -> Option<Result<ChunkText, ProcessError>> {
        None
    }

    /// Process a phrase whose start is already recognized in chunks
    ///
    /// The default implementation cannot handle chunks, so it never gets a prefix: the daemon
    /// gives it the whole recording
    fn process_with_prefix(
        &mut self,
        _prefix: ChunkPrefix,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, ProcessError> {
        self.process(audio, mode, style, app_hint)
    }
}

/// Settings that the processor needs from the config
#[derive(Debug, Clone)]
pub struct ProcessorConfig {
    pub output: OutputConfig,
    pub stt: SttOptions,
    pub default_style: String,
    /// Window class → style identifier
    pub style_by_app: std::collections::BTreeMap<String, String>,
    /// `false` means privacy mode: the journal gets a line without text
    pub include_text: bool,
    pub session_id: Uuid,
}

impl ProcessorConfig {
    pub fn from_config(config: &Config, session_id: Uuid) -> Self {
        Self {
            output: config.output.clone(),
            stt: SttOptions {
                language: LanguageHint::parse(&config.stt.language),
                allowed_languages: config.stt.allowed_languages.clone(),
                initial_prompt: None,
                threads: config.stt.threads as usize,
                timestamps: false,
            },
            default_style: config.style.default.clone(),
            style_by_app: config.style.by_app.clone(),
            include_text: config.journal.include_text && !config.privacy.no_record_mode,
            session_id,
        }
    }

    /// Style for a phrase: an explicit argument beats the binding to the application
    fn style_for(&self, style: Option<&str>, app_hint: Option<&str>) -> String {
        if let Some(style) = style.filter(|s| !s.is_empty()) {
            return style.to_string();
        }
        if let Some(app) = app_hint {
            if let Some(found) = self.style_by_app.get(app) {
                return found.clone();
            }
        }
        self.default_style.clone()
    }
}

/// Processor without rules and model: recognize, paste, record
///
/// This is enough for an end-to-end demo and for the daemon tests. The pipeline with rules and
/// an LLM takes the same place by implementing `Processor`
#[derive(Debug)]
pub struct SimpleProcessor<I: TextInjector, J: Journal> {
    stt: Box<dyn SttEngine>,
    injector: I,
    journal: J,
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn Notifier>,
    config: ProcessorConfig,
    /// Daemon measurement: from the key release to the close of the microphone stream
    stop_after_release_ms: Option<u32>,
}

impl<I: TextInjector, J: Journal> SimpleProcessor<I, J> {
    pub fn new(
        stt: Box<dyn SttEngine>,
        injector: I,
        journal: J,
        clock: Arc<dyn Clock>,
        notifier: Arc<dyn Notifier>,
        config: ProcessorConfig,
    ) -> Self {
        Self {
            stt,
            injector,
            journal,
            clock,
            notifier,
            config,
            stop_after_release_ms: None,
        }
    }
}

/// Milliseconds between two points of monotonic time, with no overflow
fn ms_between(from: Instant, to: Instant) -> u32 {
    u32::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u32::MAX)
}

impl<I: TextInjector, J: Journal> Processor for SimpleProcessor<I, J> {
    fn process(
        &mut self,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, ProcessError> {
        let started = self.clock.instant();
        let audio = audio.to_16k();
        let audio_secs = audio.duration_secs();

        let stt_engine = self.stt.id().to_string();
        let stt_model = self.stt.model_name().to_string();

        let before_stt = self.clock.instant();
        let transcript = self.stt.transcribe(&audio, &self.config.stt)?;
        let t_stt = ms_between(before_stt, self.clock.instant());

        let text = transcript.text.trim().to_string();
        let mut inject_method = None;
        let mut t_inject = None;
        let mut error = None;

        if text.is_empty() {
            // Silence or a model failure: there is no phrase, but the entry stays in the journal
            error = Some("speech not recognized".to_string());
            self.notifier
                .notify("MolvAI", "speech not recognized, try again");
        } else {
            let resolved = parse_output_mode(&self.config.output.mode)
                .resolve(&text, self.config.output.auto_type_max_chars as usize);
            let before_inject = self.clock.instant();
            // The paste method depends on the application: a terminal needs Ctrl+Shift+V
            self.injector.set_window(app_hint);
            match self.injector.inject(&text, resolved) {
                Ok(report) => inject_method = Some(report.method),
                Err(err) => {
                    tracing::warn!(%err, "paste failed");
                    error = Some(err.to_string());
                    self.notifier
                        .notify("MolvAI", &inject_failure_hint(&err, &text));
                }
            }
            t_inject = Some(ms_between(before_inject, self.clock.instant()));
        }

        let words = word_count(&text);
        let entry = Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            ts: self.clock.now_utc(),
            session_id: self.config.session_id,
            mode,
            source: Source::Mic,
            app: app_hint.map(str::to_string),
            language: transcript.detected_language.clone(),
            audio_secs,
            words,
            wpm: Entry::wpm_for(words, audio_secs),
            style: self.config.style_for(style, app_hint),
            stt_engine,
            stt_model,
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method,
            latency_ms: LatencyMs {
                stt: t_stt,
                rules: 0,
                llm: None,
                inject: t_inject,
                total: ms_between(started, self.clock.instant()),
                first_hypothesis: None,
                stop_after_release: self.stop_after_release_ms.take(),
            },
            tokens: None,
            error,
            text_raw: Some(transcript.text.clone()),
            text_final: Some(text),
            audio_path: None,
        };

        let stored = if self.config.include_text {
            entry.clone()
        } else {
            entry.clone().without_text()
        };
        self.journal.append(&stored)?;
        Ok(entry)
    }

    fn set_stop_after_release(&mut self, ms: u32) {
        self.stop_after_release_ms = Some(ms);
    }
}

/// What to tell the user when the paste failed
fn inject_failure_hint(err: &InjectError, text: &str) -> String {
    match err {
        InjectError::UnsupportedCharacters => {
            "the text cannot be typed this way; it is in the clipboard, press Ctrl+V".into()
        }
        _ => format!(
            "cannot paste the text ({} chars); it is in the clipboard, press Ctrl+V",
            text.chars().count()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::{
        FakeClock, FakeStt, MemJournal, RecordingInjector, RecordingNotifier,
    };
    use crate::domain::inject::{InjectReport, OutputMode};
    use crate::domain::stt::Transcript;
    use chrono::{DateTime, Utc};
    use std::time::Duration;

    /// A recognizer that "spends" the time of the fake clock, so the timings in the entry are
    /// not zero
    #[derive(Debug)]
    struct SlowStt {
        inner: FakeStt,
        clock: Arc<FakeClock>,
        takes: Duration,
    }

    impl SttEngine for SlowStt {
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
            self.clock.advance(self.takes);
            self.inner.transcribe(audio, opts)
        }
        fn unload(&mut self) {
            self.inner.unload();
        }
    }

    fn clock() -> Arc<FakeClock> {
        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        Arc::new(FakeClock::at(start))
    }

    fn config() -> ProcessorConfig {
        ProcessorConfig::from_config(&Config::default(), Uuid::nil())
    }

    fn audio() -> PcmAudio {
        PcmAudio::new(vec![0.2; 32_000], 16_000)
    }

    #[test]
    fn transcribed_text_reaches_the_injector_and_the_journal() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("привет мир"),
            clock: clock.clone(),
            takes: Duration::from_millis(40),
        };
        let notifier = Arc::new(RecordingNotifier::default());
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            notifier,
            config(),
        );

        let entry = processor
            .process(audio(), Mode::Dictation, None, Some("kitty"))
            .unwrap();

        assert_eq!(
            processor.injector.injected,
            vec![("привет мир".to_string(), OutputMode::Type)],
            "the injector must get exactly the transcript text"
        );
        assert_eq!(processor.journal.entries.len(), 1);
        let stored = &processor.journal.entries[0];
        assert_eq!(stored.text_final.as_deref(), Some("привет мир"));
        assert_eq!(stored.words, 2);
        assert_eq!(stored.app.as_deref(), Some("kitty"));
        assert!(stored.latency_ms.total > 0, "timings are not filled");
        assert!(stored.latency_ms.stt >= 40);
        assert_eq!(stored.error, None);
        assert_eq!(entry.id, stored.id);
    }

    #[test]
    fn wpm_is_computed_from_audio_length() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("раз два три четыре"),
            clock: clock.clone(),
            takes: Duration::from_millis(10),
        };
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            Arc::new(RecordingNotifier::default()),
            config(),
        );
        // Two seconds of audio for four words is 120 words per minute
        let entry = processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(entry.audio_secs, 2.0);
        assert_eq!(entry.wpm, Some(120.0));
    }

    #[test]
    fn long_text_goes_through_paste_not_typing() {
        let clock = clock();
        let long = "слово ".repeat(60);
        let stt = SlowStt {
            inner: FakeStt::returning(long.trim()),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            Arc::new(RecordingNotifier::default()),
            config(),
        );
        processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(processor.injector.injected[0].1, OutputMode::Paste);
    }

    #[test]
    fn failed_injection_still_produces_an_entry_with_an_error_and_a_notification() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("текст"),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let injector = RecordingInjector {
            fail_with: Some(InjectError::Failed("no active window".into())),
            ..RecordingInjector::default()
        };
        let notifier = Arc::new(RecordingNotifier::default());
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            injector,
            MemJournal::default(),
            clock.clone(),
            notifier.clone(),
            config(),
        );

        let entry = processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap();
        assert!(
            entry.error.is_some(),
            "a paste failure must reach the entry"
        );
        assert_eq!(entry.text_final.as_deref(), Some("текст"));
        assert_eq!(processor.journal.entries.len(), 1);
        let messages = notifier.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].1.contains("Ctrl+V"), "{:?}", messages[0]);
    }

    #[test]
    fn stt_failure_is_reported_as_an_error_and_writes_nothing() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::with_responses(vec![Err(SttError::Inference(
                "the model crashed".into(),
            ))]),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            Arc::new(RecordingNotifier::default()),
            config(),
        );
        let err = processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap_err();
        assert!(matches!(err, ProcessError::Stt(_)), "{err}");
        assert!(processor.journal.entries.is_empty());
        assert!(processor.injector.injected.is_empty());
    }

    #[test]
    fn empty_transcript_injects_nothing() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("   "),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let notifier = Arc::new(RecordingNotifier::default());
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            notifier.clone(),
            config(),
        );
        let entry = processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap();
        assert!(processor.injector.injected.is_empty());
        assert_eq!(entry.words, 0);
        assert!(entry.error.is_some());
        assert_eq!(notifier.messages.lock().unwrap().len(), 1);
    }

    #[test]
    fn privacy_mode_keeps_text_out_of_the_journal() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("секрет"),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let mut config = config();
        config.include_text = false;
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            Arc::new(RecordingNotifier::default()),
            config,
        );
        let entry = processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(entry.text_final.as_deref(), Some("секрет"));
        assert_eq!(processor.journal.entries[0].text_final, None);
        assert_eq!(processor.journal.entries[0].words, 1);
    }

    /// An injector that remembers which window was named to it before the paste
    #[derive(Debug, Default)]
    struct WindowAwareInjector {
        window: Option<String>,
        window_set_before_inject: bool,
    }

    impl TextInjector for WindowAwareInjector {
        fn id(&self) -> &'static str {
            "window-aware"
        }
        fn available(&self) -> bool {
            true
        }
        fn set_window(&mut self, class: Option<&str>) {
            self.window = class.map(str::to_string);
        }
        fn inject(&mut self, _text: &str, _mode: OutputMode) -> Result<InjectReport, InjectError> {
            self.window_set_before_inject = self.window.is_some();
            Ok(InjectReport {
                method: "window-aware".into(),
                attempts: Vec::new(),
            })
        }
    }

    #[test]
    fn the_active_window_is_named_to_the_injector_before_the_paste() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("текст"),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            WindowAwareInjector::default(),
            MemJournal::default(),
            clock.clone(),
            Arc::new(RecordingNotifier::default()),
            config(),
        );
        processor
            .process(audio(), Mode::Dictation, None, Some("kitty"))
            .unwrap();
        assert_eq!(processor.injector.window.as_deref(), Some("kitty"));
        assert!(
            processor.injector.window_set_before_inject,
            "the window must be known before the paste, not after"
        );
    }

    #[test]
    fn style_comes_from_the_argument_then_from_the_app_binding() {
        let mut config = config();
        config
            .style_by_app
            .insert("Slack".into(), "casual".to_string());
        assert_eq!(config.style_for(Some("formal"), Some("Slack")), "formal");
        assert_eq!(config.style_for(None, Some("Slack")), "casual");
        assert_eq!(config.style_for(None, Some("kitty")), "cleanup");
        assert_eq!(config.style_for(None, None), "cleanup");
    }

    #[test]
    fn injector_report_method_lands_in_the_entry() {
        let clock = clock();
        let stt = SlowStt {
            inner: FakeStt::returning("текст"),
            clock: clock.clone(),
            takes: Duration::from_millis(5),
        };
        let mut processor = SimpleProcessor::new(
            Box::new(stt),
            RecordingInjector::default(),
            MemJournal::default(),
            clock.clone(),
            Arc::new(RecordingNotifier::default()),
            config(),
        );
        let entry = processor
            .process(audio(), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(entry.inject_method.as_deref(), Some("recording-type"));
        let _ = InjectReport::default();
    }
}
