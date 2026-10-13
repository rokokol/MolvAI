// SPDX-License-Identifier: MIT
//! Daemon: the only place where hotkeys, IPC, microphone and pipeline meet
//!
//! The design is simple and single-threaded in its decisions on purpose. Commands from all
//! sources go into one channel, and the control thread with the state machine takes them. Long
//! processing goes to a separate worker thread and returns through the same channel. So "busy"
//! is not a race but a state of the machine, and a recording cannot start twice

pub mod chunked;
pub mod processor;
pub mod state;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::config::Config;
use crate::domain::audio::{AudioSource, PcmAudio};
use crate::domain::clock::Clock;
use crate::domain::entry::Mode;
use crate::domain::inject::{OutputMode, TextInjector};
use crate::domain::notify::Notifier;
use crate::domain::sound::{CueKind, SoundCue};
use crate::infra::audio::level::ZeroLevelWatch;
use crate::infra::ipc::RequestHandler;
use crate::infra::platform;
use crate::ipc::protocol::{Command, DaemonState, ErrorCode, Event, IpcError};

/// What to do when the microphone gives a digital zero. On macOS a common cause is denied
/// access: the system returns silence, not an error
#[cfg(target_os = "macos")]
const SILENT_MICROPHONE_HINT: &str =
    "check access: System Settings → Privacy & Security → Microphone";
#[cfg(not(target_os = "macos"))]
const SILENT_MICROPHONE_HINT: &str = "check the device: molva devices";

pub use chunked::{ChunkAccumulator, ChunkContext, ChunkFeeder, ChunkPrefix, ChunkText};
pub use processor::{ProcessError, Processor, ProcessorConfig, SimpleProcessor};
pub use state::{Action, DiscardReason, Input, Machine, Outcome};

/// Command for the control thread and, if needed, a channel for the reply
struct Message {
    input: Input,
    reply: Option<Sender<Outcome>>,
}

/// Everything that reaches the control thread
///
/// The tick of streaming processing does not pass through the state machine: it decides
/// nothing and only takes fresh sound from the microphone, which this same thread owns
enum Ctl {
    Message(Message),
    ChunkTick {
        generation: u64,
    },
    /// Reloaded settings: hotkeys for the state machine, the rest for the worker thread
    Reload(Box<Config>),
}

/// Job for the worker thread
enum Job {
    /// Reloaded settings for the pipeline: the default style, the rules, the thresholds
    Reload(Box<Config>),
    /// A chunk of a recording that is still running: it is recognized while the person speaks
    Chunk {
        audio: PcmAudio,
        /// Start of the recording: the delay to the first hypothesis counts from it
        started: Instant,
    },
    /// The phrase has ended
    Finish {
        /// The whole phrase: used when there were no chunks or they did not add up
        audio: PcmAudio,
        /// Tail after the last chunk
        tail: Option<PcmAudio>,
        mode: Mode,
        style: Option<String>,
        app: Option<String>,
        /// From the key release to the actual close of the microphone stream, in milliseconds
        stop_after_release_ms: u32,
    },
    /// The recording was cancelled: forget the collected chunks
    Discard,
}

/// Milliseconds between two points of monotonic time, with no overflow
fn millis_between(from: Instant, to: Instant) -> u32 {
    u32::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u32::MAX)
}

/// Everything the daemon needs from outside. The hardware comes as traits, so fakes can test
/// the whole daemon
#[derive(Debug)]
pub struct DaemonParts {
    pub audio: Box<dyn AudioSource>,
    pub processor: Box<dyn Processor>,
    pub notifier: Arc<dyn Notifier>,
    /// Sound cues for the start and the end of recording; `NullSoundCue` makes the silence,
    /// not a missing call
    pub sound: Arc<dyn SoundCue>,
    /// Paste method for ready text (`inject.text`): the repeat of a phrase from the history
    ///
    /// An instance separate from the pipeline: the pipeline is busy with its own phrase, and a
    /// repeat from the history arrives over IPC at any moment and must not wait for recognition
    pub injector: Option<Box<dyn TextInjector>>,
    pub clock: Arc<dyn Clock>,
    pub config: Config,
}

/// Shared state of the daemon, visible to all handles
#[derive(Debug)]
struct Shared {
    tx: Mutex<Sender<Ctl>>,
    state: Mutex<DaemonState>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    session_id: Uuid,
    started_at: DateTime<Utc>,
    /// Number of the current recording: the duration watchdog must not cut the next one
    generation: AtomicU64,
    /// Paste of ready text by `inject.text`
    injector: Mutex<Option<Box<dyn TextInjector>>>,
    /// Threshold `output.auto_type_max_chars` for resolving the `auto` mode
    auto_type_max_chars: u32,
    /// Settings file for `config.reload`; `None` means the default path
    config_path: Mutex<Option<PathBuf>>,
}

impl Shared {
    /// Send an event to the subscribers; those that dropped off unsubscribe by themselves
    fn publish(&self, event: &Event) {
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn set_state(&self, state: DaemonState, mode: Option<Mode>) {
        let changed = match self.state.lock() {
            Ok(mut current) if *current != state => {
                *current = state;
                true
            }
            _ => false,
        };
        if changed {
            self.publish(&Event::State { state, mode });
        }
    }
}

/// A running daemon. While this value lives, its threads live too
#[derive(Debug)]
pub struct Daemon {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Handle of the daemon: it is given to the IPC server and to the hotkey sources
#[derive(Debug, Clone)]
pub struct DaemonHandle {
    shared: Arc<Shared>,
}

impl Daemon {
    /// Start the daemon: the control thread, the worker thread and the signal level thread
    // The three threads and their closures read as one whole: the channels they share are
    // visible only here. Splitting them into functions belongs together with the track of
    // streaming processing, which changes these same closures now
    #[allow(clippy::too_many_lines)]
    pub fn spawn(parts: DaemonParts) -> Daemon {
        let DaemonParts {
            mut audio,
            mut processor,
            notifier,
            sound,
            injector,
            clock,
            config,
        } = parts;

        let (tx, rx) = channel::<Ctl>();
        let (work_tx, work_rx) = channel::<Job>();
        let (level_tx, level_rx) = channel::<f32>();

        let shared = Arc::new(Shared {
            tx: Mutex::new(tx.clone()),
            state: Mutex::new(DaemonState::Idle),
            subscribers: Mutex::new(Vec::new()),
            session_id: Uuid::new_v4(),
            started_at: clock.now_utc(),
            generation: AtomicU64::new(0),
            injector: Mutex::new(injector),
            auto_type_max_chars: config.output.auto_type_max_chars,
            config_path: Mutex::new(None),
        });

        let mut machine = Machine::new(config.hotkeys.clone(), clock.clone());
        let max_duration = Duration::from_secs(u64::from(config.audio.max_duration_secs));
        let mut feeder = ChunkFeeder::new(&config.stt, &config.audio);
        let streaming_preview = config.stt.streaming_preview;

        let mut threads = Vec::new();

        // Control thread: the only one that touches the state machine and the microphone
        {
            let shared = shared.clone();
            let notifier = notifier.clone();
            let sound = sound.clone();
            let tx = tx.clone();
            let control_clock = clock.clone();
            let clock = clock.clone();
            let work_tx = work_tx.clone();
            threads.push(std::thread::spawn(move || {
                let mut pending: Option<(Mode, Option<String>, Option<String>)> = None;
                while let Ok(ctl) = rx.recv() {
                    let message = match ctl {
                        // Streaming tick: take fresh sound and send the matured chunks to
                        // recognition, without waiting for the key release
                        Ctl::ChunkTick { generation } => {
                            if shared.generation.load(Ordering::SeqCst) == generation {
                                let started = feeder.started();
                                for chunk in feeder.tick(audio.as_mut()) {
                                    let job = Job::Chunk {
                                        audio: chunk.audio,
                                        started,
                                    };
                                    if work_tx.send(job).is_err() {
                                        break;
                                    }
                                }
                            }
                            continue;
                        }
                        Ctl::Reload(config) => {
                            machine.set_hotkeys(config.hotkeys.clone());
                            let _ = work_tx.send(Job::Reload(config));
                            shared.publish(&Event::ConfigReloaded);
                            continue;
                        }
                        Ctl::Message(message) => message,
                    };
                    // Start of the count for the guarantee "microphone freed after the phrase":
                    // from the stop command (that is, from the key release) to the stream
                    // close (AG-03)
                    let released_from = control_clock.instant();
                    let input_label = format!("{:?}", message.input);
                    let outcome = machine.on(message.input);
                    tracing::debug!(
                        input = %input_label,
                        actions = ?outcome.actions,
                        state = ?machine.state(),
                        in_flight = machine.in_flight(),
                        "state machine"
                    );
                    for action in &outcome.actions {
                        match action {
                            Action::StartCapture { mode, style } => {
                                let app = platform::active_window_class();
                                match audio.start(Some(level_tx.clone())) {
                                    Ok(()) => {
                                        // The hold counts from the open microphone: the stream
                                        // start itself takes up to 300 ms, and a short tap would
                                        // otherwise look like a long hold
                                        machine.capture_started(control_clock.instant());
                                        pending = Some((*mode, style.clone(), app));
                                        // The first of the two cues of a phrase: the microphone
                                        // is open
                                        sound.play(CueKind::RecordStart);
                                        shared.set_state(DaemonState::Recording, Some(*mode));
                                        let generation =
                                            shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
                                        spawn_max_duration_guard(
                                            tx.clone(),
                                            shared.clone(),
                                            generation,
                                            max_duration,
                                        );
                                        feeder.start(clock.instant());
                                        if feeder.is_enabled() {
                                            spawn_chunk_ticker(
                                                tx.clone(),
                                                shared.clone(),
                                                generation,
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        // The microphone did not open: the machine must not stay
                                        // in recording
                                        sound.play(CueKind::Error);
                                        machine.on(Input::RecordCancel);
                                        shared.set_state(DaemonState::Idle, None);
                                        shared.publish(&Event::Error {
                                            code: ErrorCode::NoDevice,
                                            message: err.to_string(),
                                            hint: Some("check the device: molva doctor".into()),
                                        });
                                        notifier.notify("MolvAI", &err.to_string());
                                    }
                                }
                            }
                            Action::StopCaptureAndProcess { mode, style } => {
                                shared.generation.fetch_add(1, Ordering::SeqCst);
                                let app = pending.take().and_then(|(_, _, app)| app);
                                match audio.stop() {
                                    Ok(pcm) => {
                                        // The second and last cue of a phrase: the microphone is
                                        // closed
                                        sound.play(CueKind::RecordStop);
                                        let stop_after_release_ms =
                                            millis_between(released_from, control_clock.instant());
                                        tracing::info!(
                                            ms = stop_after_release_ms,
                                            "microphone released"
                                        );
                                        shared.set_state(DaemonState::Transcribing, Some(*mode));
                                        // Chunks returned during recording; if there were none,
                                        // the phrase goes whole, as before
                                        let streamed = feeder.emitted();
                                        let started = feeder.started();
                                        let mut rest = feeder.finish(&pcm);
                                        let tail = if streamed == 0 {
                                            rest.clear();
                                            None
                                        } else {
                                            rest.pop().map(|chunk| chunk.audio)
                                        };
                                        let mut sent = true;
                                        for chunk in rest {
                                            let job = Job::Chunk {
                                                audio: chunk.audio,
                                                started,
                                            };
                                            sent &= work_tx.send(job).is_ok();
                                        }
                                        let job = Job::Finish {
                                            audio: pcm,
                                            tail,
                                            mode: *mode,
                                            style: style.clone(),
                                            app,
                                            stop_after_release_ms,
                                        };
                                        if !sent || work_tx.send(job).is_err() {
                                            machine.on(Input::ProcessingFailed);
                                            shared.set_state(DaemonState::Idle, None);
                                        }
                                    }
                                    Err(err) => {
                                        sound.play(CueKind::Error);
                                        machine.on(Input::ProcessingFailed);
                                        shared.set_state(DaemonState::Idle, None);
                                        shared.publish(&Event::Error {
                                            code: ErrorCode::NoDevice,
                                            message: err.to_string(),
                                            hint: None,
                                        });
                                    }
                                }
                            }
                            Action::DiscardCapture { reason } => {
                                shared.generation.fetch_add(1, Ordering::SeqCst);
                                pending = None;
                                feeder.cancel();
                                // The chunks of a cancelled recording must not attach to the next
                                // phrase
                                let _ = work_tx.send(Job::Discard);
                                // The microphone may not have opened: a failure here changes
                                // nothing
                                let _ = audio.stop();
                                // There will be no phrase, so there is no end cue either: the user
                                // hears that the press went to waste
                                sound.play(CueKind::Error);
                                shared.set_state(DaemonState::Idle, None);
                                tracing::info!(reason = reason.message(), "recording discarded");
                            }
                        }
                    }
                    if outcome.actions.is_empty() {
                        shared.set_state(machine.state(), None);
                    }
                    if let Some(reply) = message.reply {
                        let _ = reply.send(outcome);
                    }
                }
            }));
        }

        // Worker thread: while it is busy with recognition, the control thread keeps answering
        // "busy"
        {
            let shared = shared.clone();
            let notifier = notifier.clone();
            let tx = tx.clone();
            threads.push(std::thread::spawn(move || {
                // Chunks come in one queue to one thread, so the order of the texts is the order of
                // the chunks, and they cannot get mixed up
                let mut chunks = ChunkAccumulator::default();
                while let Ok(job) = work_rx.recv() {
                    let (audio, tail, mode, style, app, stop_after_release_ms) = match job {
                        Job::Discard => {
                            chunks.reset();
                            continue;
                        }
                        Job::Reload(config) => {
                            processor.apply_config(&config);
                            tracing::info!(style = %config.style.default, "pipeline reloaded the settings");
                            continue;
                        }
                        Job::Chunk { audio, started } => {
                            process_chunk(
                                processor.as_mut(),
                                &mut chunks,
                                &shared,
                                &audio,
                                started,
                                streaming_preview,
                            );
                            continue;
                        }
                        Job::Finish {
                            audio,
                            tail,
                            mode,
                            style,
                            app,
                            stop_after_release_ms,
                        } => (audio, tail, mode, style, app, stop_after_release_ms),
                    };
                    // The guarantee "microphone freed after the phrase" must be visible in the
                    // journal, and not only in the daemon log
                    processor.set_stop_after_release(stop_after_release_ms);
                    // There were no chunks, or none held speech: the phrase goes whole
                    let (audio, prefix) = if chunks.is_empty() {
                        chunks.reset();
                        (audio, ChunkPrefix::default())
                    } else {
                        let prefix = chunks.take_prefix(audio.duration_secs());
                        (tail.unwrap_or_default(), prefix)
                    };
                    let result = processor.process_with_prefix(
                        prefix,
                        audio,
                        mode,
                        style.as_deref(),
                        app.as_deref(),
                    );
                    let input = match result {
                        Ok(entry) => {
                            shared.publish(&Event::Entry {
                                entry: Box::new(entry),
                            });
                            Input::ProcessingDone
                        }
                        Err(err) => {
                            tracing::error!(%err, "processing failed");
                            notifier.notify("MolvAI", &err.to_string());
                            shared.publish(&Event::Error {
                                code: error_code_for(&err),
                                message: err.to_string(),
                                hint: Some("details: molva doctor".into()),
                            });
                            Input::ProcessingFailed
                        }
                    };
                    let _ = tx.send(Ctl::Message(Message { input, reply: None }));
                }
            }));
        }

        // Level thread: the indicator in the GUI and the warning about a silent microphone. The
        // watcher lives here and not in the cpal callback: a real-time callback cannot show a
        // notification, and from here it goes by the same path as a recording error
        {
            let shared = shared.clone();
            let notifier = notifier.clone();
            let clock = clock.clone();
            threads.push(std::thread::spawn(move || {
                let mut silence = ZeroLevelWatch::with_defaults();
                while let Ok(rms) = level_rx.recv() {
                    if let Some(message) = silence.observe(rms, clock.instant()) {
                        tracing::warn!("{message}");
                        notifier.notify("MolvAI", message);
                        shared.publish(&Event::Error {
                            code: ErrorCode::NoDevice,
                            message: message.to_string(),
                            hint: Some(SILENT_MICROPHONE_HINT.into()),
                        });
                    }
                    shared.publish(&Event::Level { rms });
                }
            }));
        }

        Daemon { shared, threads }
    }

    pub fn handle(&self) -> DaemonHandle {
        DaemonHandle {
            shared: self.shared.clone(),
        }
    }

    /// Wait for the threads to finish. Returns when the channels are closed
    pub fn join(self) {
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

/// Duration watchdog: cuts a recording if it was forgotten
fn spawn_max_duration_guard(tx: Sender<Ctl>, shared: Arc<Shared>, generation: u64, max: Duration) {
    if max.is_zero() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(max);
        // The generation number protects the next recording: the watchdog of the old one does
        // not touch it
        if shared.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = tx.send(Ctl::Message(Message {
            input: Input::MaxDuration,
            reply: None,
        }));
    });
}

/// Alarm of streaming processing: four times a second it asks the control thread to take
/// fresh sound
///
/// The control thread itself takes the sound, because it alone owns the microphone. The alarm
/// goes quiet as soon as the recording changes to the next one
fn spawn_chunk_ticker(tx: Sender<Ctl>, shared: Arc<Shared>, generation: u64) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(chunked::TICK_MS));
        if shared.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if tx.send(Ctl::ChunkTick { generation }).is_err() {
            return;
        }
    });
}

/// Recognize the next chunk and show the draft to the subscribers
///
/// A chunk that failed to recognize does not fail the phrase: its text is simply missing, and
/// the other chunks and the tail arrive as usual
fn process_chunk(
    processor: &mut dyn Processor,
    chunks: &mut ChunkAccumulator,
    shared: &Shared,
    audio: &PcmAudio,
    started: Instant,
    streaming_preview: bool,
) {
    let context = chunks.context();
    let Some(result) = processor.transcribe_chunk(audio, &context) else {
        // The processor cannot do streaming processing: it will get the phrase whole
        return;
    };
    match result {
        Ok(chunk) => {
            let since_start = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            chunks.push(chunk, since_start);
            if streaming_preview {
                shared.publish(&Event::Hypothesis {
                    text: chunks.draft(),
                });
            }
        }
        Err(err) => {
            tracing::warn!(%err, "phrase chunk not recognized, the others are not affected");
        }
    }
}

fn error_code_for(err: &ProcessError) -> ErrorCode {
    match err {
        ProcessError::Stt(_) => ErrorCode::SttFailed,
        ProcessError::Inject(_) => ErrorCode::InjectFailed,
        ProcessError::Journal(_) => ErrorCode::Internal,
        ProcessError::Pipeline(_) => ErrorCode::BadRequest,
    }
}

impl DaemonHandle {
    /// Send an input and wait for the decision of the state machine
    pub fn send(&self, input: Input) -> Result<Outcome, IpcError> {
        let (reply_tx, reply_rx) = channel();
        let message = Ctl::Message(Message {
            input,
            reply: Some(reply_tx),
        });
        {
            let tx = self.shared.tx.lock().map_err(|_| internal("daemon busy"))?;
            tx.send(message).map_err(|_| internal("daemon stopped"))?;
        }
        reply_rx.recv().map_err(|_| internal("daemon stopped"))
    }

    /// Send an input without waiting for a reply: for hotkey sources
    pub fn send_async(&self, input: Input) {
        if let Ok(tx) = self.shared.tx.lock() {
            let _ = tx.send(Ctl::Message(Message { input, reply: None }));
        }
    }

    pub fn state(&self) -> DaemonState {
        self.shared.state.lock().map_or(DaemonState::Idle, |s| *s)
    }

    pub fn session_id(&self) -> Uuid {
        self.shared.session_id
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.shared.started_at
    }

    /// Subscribe to the events of the daemon
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = channel();
        if let Ok(mut subscribers) = self.shared.subscribers.lock() {
            subscribers.push(tx);
        }
        rx
    }

    /// Paste a ready text: the repeat of a phrase from the history (`molva history paste`)
    ///
    /// The microphone and recognition take no part here: the text already exists, and only the
    /// same paste method as for an ordinary phrase is needed, with the correction for the class
    /// of the active window
    pub fn inject_text(
        &self,
        text: &str,
        mode: Option<OutputMode>,
    ) -> Result<serde_json::Value, IpcError> {
        if text.trim().is_empty() {
            return Err(IpcError {
                code: ErrorCode::BadRequest,
                message: "empty text, nothing to paste".into(),
                hint: None,
            });
        }
        let mut guard = self
            .shared
            .injector
            .lock()
            .map_err(|_| internal("paste method busy"))?;
        let Some(injector) = guard.as_mut() else {
            return Err(IpcError {
                code: ErrorCode::InjectFailed,
                message: "the daemon runs without a paste method".into(),
                hint: Some("you can copy the text: molva history show <id>".into()),
            });
        };
        let app = platform::active_window_class();
        injector.set_window(app.as_deref());
        let mode = mode
            .unwrap_or(OutputMode::Auto)
            .resolve(text, self.shared.auto_type_max_chars as usize);
        match injector.inject(text, mode) {
            Ok(report) => Ok(serde_json::json!({ "ok": true, "method": report.method })),
            Err(err) => Err(IpcError {
                code: ErrorCode::InjectFailed,
                message: err.to_string(),
                hint: Some("the text stayed in the clipboard, press Ctrl+V".into()),
            }),
        }
    }

    /// Summary for `molva status`
    /// Where the daemon rereads the settings on `config.reload`; without a path it is the
    /// default file
    pub fn set_config_path(&self, path: PathBuf) {
        if let Ok(mut slot) = self.shared.config_path.lock() {
            *slot = Some(path);
        }
    }

    /// Reread the settings file and send it to the state machine and to the pipeline
    /// `style` replaces the default style: this is how `style.set` works from the tray and the CLI
    pub fn reload_config(&self, style: Option<String>) -> Result<Config, IpcError> {
        let path = match self
            .shared
            .config_path
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
        {
            Some(path) => path,
            None => Config::default_path().map_err(|e| internal(&e.to_string()))?,
        };
        let mut config = Config::load(&path).map_err(|e| IpcError {
            code: ErrorCode::BadRequest,
            message: format!("settings not reloaded: {e}"),
            hint: Some("check the file: molva config validate".into()),
        })?;
        if let Some(style) = style {
            config.style.default = style;
        }
        let tx = self.shared.tx.lock().map_err(|_| internal("daemon busy"))?;
        tx.send(Ctl::Reload(Box::new(config.clone())))
            .map_err(|_| internal("daemon stopped"))?;
        Ok(config)
    }

    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state(),
            "session_id": self.session_id(),
            "started_at": self.started_at(),
            "pid": std::process::id(),
        })
    }
}

fn internal(message: &str) -> IpcError {
    IpcError {
        code: ErrorCode::Internal,
        message: message.to_string(),
        hint: None,
    }
}

fn outcome_to_result(outcome: Outcome) -> Result<serde_json::Value, IpcError> {
    match outcome.error {
        Some(err) => Err(err),
        None => Ok(serde_json::json!({ "ok": true })),
    }
}

impl RequestHandler for DaemonHandle {
    fn handle(&self, cmd: Command) -> Result<serde_json::Value, IpcError> {
        match cmd {
            Command::Ping => Ok(serde_json::json!({
                "pid": std::process::id(),
                "session_id": self.session_id(),
            })),
            Command::Status => Ok(self.status_json()),
            Command::RecordStart { mode, style } => {
                outcome_to_result(self.send(Input::RecordStart { mode, style })?)
            }
            Command::RecordStop => outcome_to_result(self.send(Input::RecordStop)?),
            Command::RecordToggle { mode, style } => {
                outcome_to_result(self.send(Input::RecordToggle { mode, style })?)
            }
            Command::RecordCancel => outcome_to_result(self.send(Input::RecordCancel)?),
            Command::InjectText { text, mode } => self.inject_text(&text, mode),
            Command::ConfigReload => {
                let config = self.reload_config(None)?;
                Ok(serde_json::json!({
                    "reloaded": true,
                    "style": config.style.default,
                }))
            }
            Command::StyleSet { style } => {
                let config = self.reload_config(Some(style))?;
                Ok(serde_json::json!({ "style": config.style.default }))
            }
            Command::Shutdown => {
                // The reply must leave before the process disappears
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(100));
                    std::process::exit(0);
                });
                Ok(serde_json::json!({ "ok": true }))
            }
            other => Err(IpcError {
                code: ErrorCode::BadRequest,
                message: format!(
                    "the daemon does not support the command {} yet",
                    command_name(&other)
                ),
                hint: Some(
                    "build of track F: ping, status, record.*, subscribe are available".into(),
                ),
            }),
        }
    }

    fn subscribe(&self, _levels: bool) -> Option<Receiver<Event>> {
        Some(DaemonHandle::subscribe(self))
    }
}

/// Name of a command from the protocol, for error messages
fn command_name(cmd: &Command) -> String {
    serde_json::to_value(cmd)
        .ok()
        .and_then(|v| v.get("cmd").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::{
        FakeAudioSource, FakeClock, FakeStt, MemJournal, RecordingNotifier, RecordingSoundCue,
    };
    use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
    use crate::infra::audio::level::ZERO_LEVEL_MESSAGE;
    use std::time::Duration;

    /// An injector whose result stays visible to the test after it goes into the daemon
    #[derive(Debug, Clone, Default)]
    struct SharedInjector {
        injected: Arc<Mutex<Vec<String>>>,
        fail: bool,
        /// Paste delay: this is how the test keeps the daemon in processing while it sends new
        /// presses
        delay: Duration,
    }

    impl TextInjector for SharedInjector {
        fn id(&self) -> &'static str {
            "shared"
        }
        fn available(&self) -> bool {
            true
        }
        fn inject(&mut self, text: &str, _mode: OutputMode) -> Result<InjectReport, InjectError> {
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            if self.fail {
                return Err(InjectError::Failed("no active window".into()));
            }
            self.injected.lock().unwrap().push(text.to_string());
            Ok(InjectReport {
                method: "shared".into(),
                attempts: Vec::new(),
            })
        }
    }

    /// A microphone that the test keeps watching after it hands it to the daemon
    #[derive(Debug, Clone)]
    struct SharedAudio(Arc<Mutex<FakeAudioSource>>);

    impl AudioSource for SharedAudio {
        fn start(
            &mut self,
            level_tx: Option<Sender<f32>>,
        ) -> Result<(), crate::domain::audio::AudioError> {
            self.0.lock().unwrap().start(level_tx)
        }
        fn stop(&mut self) -> Result<PcmAudio, crate::domain::audio::AudioError> {
            self.0.lock().unwrap().stop()
        }
        fn is_recording(&self) -> bool {
            self.0.lock().unwrap().is_recording()
        }
        /// Streaming processing takes fresh sound through this method: the wrapper must forward
        /// it, or no chunks appear during recording
        fn drain_new_samples(&mut self) -> Option<PcmAudio> {
            self.0.lock().unwrap().drain_new_samples()
        }
    }

    struct Harness {
        daemon: Daemon,
        handle: DaemonHandle,
        clock: Arc<FakeClock>,
        injected: Arc<Mutex<Vec<String>>>,
        notifier: Arc<RecordingNotifier>,
        sound: Arc<RecordingSoundCue>,
        audio: SharedAudio,
    }

    fn harness(text: &str) -> Harness {
        harness_with(text, false, Config::default(), Duration::ZERO)
    }

    fn harness_with(
        text: &str,
        fail_inject: bool,
        config: Config,
        inject_delay: Duration,
    ) -> Harness {
        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = Arc::new(FakeClock::at(start));
        let notifier = Arc::new(RecordingNotifier::default());
        let injector = SharedInjector {
            injected: Arc::new(Mutex::new(Vec::new())),
            fail: fail_inject,
            delay: inject_delay,
        };
        let injected = injector.injected.clone();
        let processor = SimpleProcessor::new(
            Box::new(FakeStt::returning(text)),
            injector,
            MemJournal::default(),
            clock.clone(),
            notifier.clone(),
            ProcessorConfig::from_config(&config, Uuid::nil()),
        );
        let sound = Arc::new(RecordingSoundCue::default());
        let audio = SharedAudio(Arc::new(Mutex::new(FakeAudioSource::silence(2.0))));
        let daemon = Daemon::spawn(DaemonParts {
            audio: Box::new(audio.clone()),
            processor: Box::new(processor),
            notifier: notifier.clone(),
            sound: sound.clone(),
            // A repeat from the history is pasted by the same fake: the test sees both phrases
            // and repeats
            injector: Some(Box::new(SharedInjector {
                injected: injected.clone(),
                fail: fail_inject,
                delay: inject_delay,
            })),
            clock: clock.clone(),
            config,
        });
        let handle = daemon.handle();
        Harness {
            daemon,
            handle,
            clock,
            injected,
            notifier,
            sound,
            audio,
        }
    }

    fn wait_for_entry(events: &Receiver<Event>) -> crate::domain::entry::Entry {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Entry { entry }) => return *entry,
                Ok(Event::Error { message, .. }) => {
                    panic!("the daemon reported an error: {message}")
                }
                Ok(_) | Err(_) => {}
            }
        }
        panic!("the phrase never appeared");
    }

    #[test]
    fn a_recording_ends_with_the_text_in_the_injector_and_in_an_entry() {
        let h = harness("тестовая реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert_eq!(h.handle.state(), DaemonState::Recording);
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();

        let entry = wait_for_entry(&events);
        assert_eq!(entry.text_final.as_deref(), Some("тестовая реплика"));
        assert_eq!(
            *h.injected.lock().unwrap(),
            vec!["тестовая реплика".to_string()],
            "the text must reach the injector"
        );
        assert_eq!(entry.words, 2);
        drop(h.daemon);
    }

    /// Wait for the next level measurement; everything that came before it stays in `seen`
    fn wait_for_level(events: &Receiver<Event>, seen: &mut Vec<Event>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(event) = events.recv_timeout(Duration::from_millis(200)) {
                let is_level = matches!(event, Event::Level { .. });
                seen.push(event);
                if is_level {
                    return;
                }
            }
        }
        panic!("the level measurement never came");
    }

    #[test]
    fn a_silent_microphone_is_reported_to_the_user_once_per_silence() {
        let h = harness("тестовая реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        // The level thread reads the clock when it takes a measurement, so the clock moves only
        // after the previous measurement is counted. Measurements at 0, 0.6, 1.2 and 1.8 s: the
        // warning comes on the third, and the fourth does not repeat it
        let mut seen = Vec::new();
        wait_for_level(&events, &mut seen);
        for _ in 0..3 {
            h.clock.advance(Duration::from_millis(600));
            h.audio.0.lock().unwrap().send_level(0.0);
            wait_for_level(&events, &mut seen);
        }

        let errors: Vec<(ErrorCode, String)> = seen
            .iter()
            .filter_map(|event| match event {
                Event::Error { code, message, .. } => Some((*code, message.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            errors,
            vec![(ErrorCode::NoDevice, ZERO_LEVEL_MESSAGE.to_string())]
        );
        let notified: Vec<String> = h
            .notifier
            .messages
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body)| body.clone())
            .collect();
        assert_eq!(
            notified,
            vec![ZERO_LEVEL_MESSAGE.to_string()],
            "one notification per segment of silence"
        );
        drop(h.daemon);
    }

    #[test]
    fn a_short_tap_through_ipc_latches_the_recording_instead_of_finishing_it() {
        // The compositor bind sends `record start` on press and `record stop` on release: a
        // short tap arrives as two requests in a row and must turn on hands-free, and not
        // return an empty phrase
        let h = harness("тестовая реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_millis(50));
        let outcome = h.handle.send(Input::RecordStop).unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(
            h.handle.state(),
            DaemonState::Recording,
            "after a short tap the recording continues without the key"
        );

        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        let entry = wait_for_entry(&events);
        assert_eq!(entry.text_final.as_deref(), Some("тестовая реплика"));
        drop(h.daemon);
    }

    #[test]
    fn a_press_during_processing_starts_the_next_recording_and_both_replies_arrive_in_order() {
        // The paste keeps the daemon in processing for 400 ms; the next phrase records without
        // waiting for it
        let h = harness_with(
            "тестовая реплика",
            false,
            Config::default(),
            Duration::from_millis(400),
        );
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();

        let second = h
            .handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert!(second.is_ok(), "{second:?}");
        assert_eq!(
            h.handle.state(),
            DaemonState::Recording,
            "the recording of the next phrase runs while the first one is in processing"
        );
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();

        let first = wait_for_entry(&events);
        let second = wait_for_entry(&events);
        assert_eq!(first.text_final.as_deref(), Some("тестовая реплика"));
        assert_eq!(second.text_final.as_deref(), Some("тестовая реплика"));
        assert!(first.ts <= second.ts, "phrases are processed in order");
        assert_eq!(
            h.injected.lock().unwrap().len(),
            2,
            "both phrases reached the paste step"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while h.handle.state() != DaemonState::Idle && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.handle.state(), DaemonState::Idle);
        drop(h.daemon);
    }

    #[test]
    fn style_set_and_config_reload_reread_the_file_and_answer_with_the_style() {
        let h = harness("тестовая реплика");
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.toml");
        std::fs::write(&config_path, "[style]\ndefault = \"verbatim\"\n").unwrap();
        h.handle.set_config_path(config_path.clone());
        let events = h.handle.subscribe();

        let reloaded = h.handle.handle(Command::ConfigReload).unwrap();
        assert_eq!(reloaded["style"], "verbatim");

        let set = h
            .handle
            .handle(Command::StyleSet {
                style: "messenger".into(),
            })
            .unwrap();
        assert_eq!(set["style"], "messenger");
        let saw_reload = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(2)).ok())
            .take(6)
            .any(|event| matches!(event, Event::ConfigReloaded));
        assert!(saw_reload, "subscribers learn about the reloaded settings");

        // A broken file gives an error with a hint, and not a silent "reloaded"
        std::fs::write(&config_path, "[style\n").unwrap();
        let err = h.handle.handle(Command::ConfigReload).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.hint.is_some());
        drop(h.daemon);
    }

    #[test]
    fn the_microphone_is_free_outside_a_recording_and_the_delay_lands_in_the_entry() {
        // Criterion AG-01/AG-03: outside a recording the microphone is not busy, and the time of
        // its release after the key release is visible in the journal, not only on the
        // developer's word
        let h = harness("реплика");
        assert!(
            !h.audio.is_recording(),
            "before recording the microphone is free"
        );
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert!(
            h.audio.is_recording(),
            "during recording the microphone is busy"
        );
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        let entry = wait_for_entry(&events);
        assert!(
            !h.audio.is_recording(),
            "after the phrase the microphone must be free"
        );
        let released = entry
            .latency_ms
            .stop_after_release
            .expect("the microphone release measurement must reach the entry");
        assert!(
            released < 500,
            "the microphone is released at once: {released} ms"
        );
        drop(h.daemon);
    }

    #[test]
    fn a_cancelled_recording_frees_the_microphone_too() {
        let h = harness("реплика");
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(1));
        h.handle.send(Input::RecordCancel).unwrap();
        assert!(!h.audio.is_recording());
        drop(h.daemon);
    }

    #[test]
    fn a_reply_is_framed_by_exactly_two_sound_cues() {
        // Criterion AG-05: the cue for the start of recording and the cue for the end are
        // exactly two per phrase
        let h = harness("реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        wait_for_entry(&events);
        assert_eq!(
            h.sound.played(),
            vec![CueKind::RecordStart, CueKind::RecordStop],
            "each phrase must get exactly two cues"
        );
        drop(h.daemon);
    }

    #[test]
    fn a_cancelled_recording_never_sounds_the_end_of_a_reply() {
        let h = harness("реплика");
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(1));
        h.handle.send(Input::RecordCancel).unwrap();
        assert_eq!(
            h.sound.played(),
            vec![CueKind::RecordStart, CueKind::Error],
            "a cancelled recording must not sound like a finished phrase"
        );
        drop(h.daemon);
    }

    #[test]
    fn the_daemon_returns_to_idle_after_a_reply() {
        let h = harness("реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        wait_for_entry(&events);
        // The control thread returns to Idle after the event: wait for it
        let deadline = Instant::now() + Duration::from_secs(5);
        while h.handle.state() != DaemonState::Idle && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(h.handle.state(), DaemonState::Idle);
        drop(h.daemon);
    }

    #[test]
    fn a_second_start_while_recording_is_refused_as_busy() {
        let h = harness("реплика");
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(1));
        let outcome = h
            .handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert_eq!(outcome.error.unwrap().code, ErrorCode::Busy);
        drop(h.daemon);
    }

    #[test]
    fn a_cancelled_recording_produces_no_entry() {
        let h = harness("реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordCancel).unwrap();
        assert_eq!(h.handle.state(), DaemonState::Idle);
        assert!(
            events.recv_timeout(Duration::from_millis(300)).is_err()
                || h.injected.lock().unwrap().is_empty()
        );
        assert!(h.injected.lock().unwrap().is_empty());
        drop(h.daemon);
    }

    #[test]
    fn a_failed_injection_still_yields_an_entry_and_a_notification() {
        let h = harness_with("реплика", true, Config::default(), Duration::ZERO);
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        let entry = wait_for_entry(&events);
        assert!(
            entry.error.is_some(),
            "a paste failure must be in the entry"
        );
        assert!(!h.notifier.messages.lock().unwrap().is_empty());
        drop(h.daemon);
    }

    #[test]
    fn state_changes_are_published_to_subscribers() {
        let h = harness("реплика");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::State { state, .. }) => {
                    seen.push(state);
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert_eq!(seen, vec![DaemonState::Recording]);
        drop(h.daemon);
    }

    #[test]
    fn a_dead_subscriber_is_dropped_without_breaking_the_others() {
        let h = harness("реплика");
        let alive = h.handle.subscribe();
        drop(h.handle.subscribe());
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert!(alive.recv_timeout(Duration::from_secs(2)).is_ok());
        drop(h.daemon);
    }

    #[test]
    fn ping_and_status_answer_over_the_request_handler() {
        let h = harness("реплика");
        let handler: &dyn RequestHandler = &h.handle;
        let ping = handler.handle(Command::Ping).unwrap();
        assert_eq!(ping["pid"], std::process::id());
        let status = handler.handle(Command::Status).unwrap();
        assert_eq!(status["state"], "idle");
        drop(h.daemon);
    }

    #[test]
    fn inject_text_repeats_a_ready_text_through_the_injector() {
        // Criterion AM-17: a phrase from the history can be pasted again, with no new dictation
        let h = harness("реплика");
        let handler: &dyn RequestHandler = &h.handle;
        let result = handler
            .handle(Command::InjectText {
                text: "собрание переносится".into(),
                mode: None,
            })
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(
            *h.injected.lock().unwrap(),
            vec!["собрание переносится".to_string()]
        );
        drop(h.daemon);
    }

    #[test]
    fn inject_text_refuses_an_empty_text() {
        let h = harness("реплика");
        let handler: &dyn RequestHandler = &h.handle;
        let err = handler
            .handle(Command::InjectText {
                text: "   ".into(),
                mode: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(h.injected.lock().unwrap().is_empty());
        drop(h.daemon);
    }

    #[test]
    fn a_failed_repeat_is_an_honest_error_with_a_next_step() {
        let h = harness_with("реплика", true, Config::default(), Duration::ZERO);
        let handler: &dyn RequestHandler = &h.handle;
        let err = handler
            .handle(Command::InjectText {
                text: "собрание переносится".into(),
                mode: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InjectFailed);
        assert!(
            err.hint.unwrap().contains("clipboard"),
            "a next step is needed"
        );
        drop(h.daemon);
    }

    #[test]
    fn an_unsupported_command_is_answered_with_its_name() {
        let h = harness("реплика");
        let handler: &dyn RequestHandler = &h.handle;
        let err = handler.handle(Command::DevicesList).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.message.contains("devices.list"), "{}", err.message);
        drop(h.daemon);
    }

    /// A daemon with the full pipeline and a microphone that returns sound in portions: this is
    /// the only way streaming processing works
    fn streaming_harness(responses: &[&str], config: Config) -> Harness {
        use crate::app::pipeline::{Pipeline, PipelineConfig};
        use crate::domain::stt::Transcript;

        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = Arc::new(FakeClock::at(start));
        let notifier = Arc::new(RecordingNotifier::default());
        let injector = SharedInjector {
            injected: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            delay: Duration::ZERO,
        };
        let injected = injector.injected.clone();
        let stt = FakeStt::with_responses(
            responses
                .iter()
                .map(|text| Ok(Transcript::text_only(*text)))
                .collect(),
        );
        let pipeline = Pipeline::new(
            Box::new(stt),
            None,
            Box::new(injector),
            Box::new(MemJournal::default()),
            clock.clone(),
            PipelineConfig::from_config(&config),
        );
        let sound = Arc::new(RecordingSoundCue::default());
        // Portions of eight seconds: two microphone ticks give chunks with no waiting in the test
        let audio = SharedAudio(Arc::new(Mutex::new(FakeAudioSource::paced(
            speech(20.0),
            8_000,
        ))));
        let daemon = Daemon::spawn(DaemonParts {
            audio: Box::new(audio.clone()),
            processor: Box::new(pipeline),
            notifier: notifier.clone(),
            sound: sound.clone(),
            injector: None,
            clock: clock.clone(),
            config,
        });
        let handle = daemon.handle();
        Harness {
            daemon,
            handle,
            clock,
            injected,
            notifier,
            sound,
            audio,
        }
    }

    /// Speech with pauses: a tone of one and a half seconds, then a second of silence
    fn speech(secs: f32) -> PcmAudio {
        let rate = 16_000usize;
        let mut samples = Vec::new();
        while samples.len() < (secs * rate as f32) as usize {
            samples.extend((0..rate * 3 / 2).map(|i| 0.5 * (i as f32 * 0.3).sin()));
            samples.resize(samples.len() + rate, 0.0);
        }
        PcmAudio::new(samples, rate as u32)
    }

    /// Wait for the phrase, and collect all drafts on the way
    fn drain_until_entry(events: &Receiver<Event>) -> (Vec<String>, crate::domain::entry::Entry) {
        drain_until_entry_within(events, Duration::from_secs(10))
    }

    fn drain_until_entry_within(
        events: &Receiver<Event>,
        wait: Duration,
    ) -> (Vec<String>, crate::domain::entry::Entry) {
        let mut drafts = Vec::new();
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Hypothesis { text }) => drafts.push(text),
                Ok(Event::Entry { entry }) => return (drafts, *entry),
                Ok(Event::Error { message, .. }) => {
                    panic!("the daemon reported an error: {message}")
                }
                Ok(_) | Err(_) => {}
            }
        }
        panic!(
            "the phrase never appeared, there were {} drafts",
            drafts.len()
        );
    }

    /// Record a phrase and return the drafts together with the final entry
    fn record(harness: &Harness) -> (Vec<String>, crate::domain::entry::Entry) {
        let events = harness.handle.subscribe();
        harness
            .handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        // The streaming tick runs four times a second: let the microphone return its portions
        std::thread::sleep(Duration::from_millis(700));
        harness.clock.advance(Duration::from_secs(20));
        harness.handle.send(Input::RecordStop).unwrap();
        drain_until_entry(&events)
    }

    #[test]
    fn chunks_are_recognised_during_the_recording_and_joined_in_order() {
        let responses = ["раз", "два", "три", "четыре", "пять"];
        let h = streaming_harness(&responses, Config::default());

        let (drafts, entry) = record(&h);

        assert!(
            drafts.len() >= 2,
            "fewer than two drafts came during recording: processing waits for the release again"
        );
        for pair in drafts.windows(2) {
            assert!(
                pair[1].starts_with(&pair[0]),
                "the draft lost the order of the chunks: {:?} → {:?}",
                pair[0],
                pair[1]
            );
        }
        let raw = entry.text_raw.clone().unwrap_or_default();
        let words: Vec<&str> = raw.split_whitespace().collect();
        assert!(
            words.len() >= 2,
            "the result is not built from chunks: {raw:?}"
        );
        assert_eq!(
            words,
            responses[..words.len()].to_vec(),
            "the chunks were joined in the wrong order: {raw:?}"
        );
        assert_eq!(
            drafts.last().map(String::as_str),
            Some(words[..words.len() - 1].join(" ").as_str()),
            "the last draft is everything except the tail"
        );
        assert!(
            entry.latency_ms.first_hypothesis.is_some(),
            "the time to the first hypothesis is not measured"
        );
        assert!(
            entry.audio_secs > 15.0,
            "the phrase duration is computed from the tail, not from the whole recording: {}",
            entry.audio_secs
        );
        assert_eq!(h.injected.lock().unwrap().len(), 1, "exactly one paste");
        drop(h.daemon);
    }

    #[test]
    fn with_chunking_off_the_reply_is_recognised_as_a_whole_after_the_release() {
        let mut config = Config::default();
        config.stt.chunked = false;
        let h = streaming_harness(&["целая реплика", "лишний вызов"], config);

        let (drafts, entry) = record(&h);

        assert!(drafts.is_empty(), "drafts with cutting off: {drafts:?}");
        assert_eq!(entry.text_raw.as_deref(), Some("целая реплика"));
        assert_eq!(entry.latency_ms.first_hypothesis, None);
        drop(h.daemon);
    }

    #[test]
    fn a_cancelled_recording_does_not_leak_its_chunks_into_the_next_reply() {
        let h = streaming_harness(&["отменённое", "новое"], Config::default());
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        h.clock.advance(Duration::from_secs(20));
        h.handle.send(Input::RecordCancel).unwrap();
        // The cancel reaches the worker thread through the same channel as the chunks: let it
        // arrive
        std::thread::sleep(Duration::from_millis(300));
        while events.try_recv().is_ok() {}

        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        h.clock.advance(Duration::from_secs(20));
        h.handle.send(Input::RecordStop).unwrap();
        let (_, entry) = drain_until_entry(&events);

        let raw = entry.text_raw.clone().unwrap_or_default();
        assert!(
            !raw.contains("отменённое"),
            "the text of the cancelled recording attached to the next phrase: {raw:?}"
        );
        drop(h.daemon);
    }

    /// Measurement on a real model: how long the person waits after the key release
    ///
    /// The microphone plays the speech in real time (one portion per tick), so exactly what the
    /// user sees is measured: from `RecordStop` to the ready entry. A run takes minutes, so it is
    /// manual: `MOLVA_TEST_MODEL=<path> cargo test --release -- --ignored real_model_streaming`
    #[test]
    #[ignore = "needs a downloaded whisper model: MOLVA_TEST_MODEL=<path> --ignored"]
    fn real_model_streaming_shortens_the_wait_after_the_release() {
        let Ok(model) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("set MOLVA_TEST_MODEL=<path to ggml-*.bin>");
        };
        for chunked in [false, true] {
            let mut config = Config::default();
            config.stt.chunked = chunked;
            let (elapsed, entry) = run_real_reply(&model, config);
            println!(
                "chunked = {chunked}: after the release {} ms | stt {} ms | to the first hypothesis \
                 {:?} | phrase {:.1} s\n  {}",
                elapsed.as_millis(),
                entry.latency_ms.stt,
                entry.latency_ms.first_hypothesis,
                entry.audio_secs,
                entry.text_final.as_deref().unwrap_or("")
            );
        }
    }

    /// One phrase through the daemon with a real model: the time from the release to the ready
    /// entry
    #[cfg(test)]
    fn run_real_reply(model: &str, config: Config) -> (Duration, crate::domain::entry::Entry) {
        use crate::app::pipeline::{Pipeline, PipelineConfig};
        use crate::domain::clock::SystemClock;
        use crate::infra::stt::WhisperEngine;

        let clock = Arc::new(SystemClock);
        let audio = concatenated_fixtures();
        let secs = audio.duration_secs();
        let pipeline = Pipeline::new(
            Box::new(WhisperEngine::new(model.into(), "test".into(), 0)),
            None,
            Box::new(SharedInjector {
                injected: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                delay: Duration::ZERO,
            }),
            Box::new(MemJournal::default()),
            clock.clone(),
            PipelineConfig::from_config(&config),
        );
        let daemon = Daemon::spawn(DaemonParts {
            // One portion per tick is playback in real time, as from a live microphone
            audio: Box::new(FakeAudioSource::paced(audio, chunked::TICK_MS as u32)),
            processor: Box::new(pipeline),
            notifier: Arc::new(RecordingNotifier::default()),
            // The measurement uses a real model: the cue sound and the paste do not matter here
            sound: Arc::new(RecordingSoundCue::default()),
            injector: None,
            clock,
            config,
        });
        let handle = daemon.handle();
        let events = handle.subscribe();
        handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        std::thread::sleep(Duration::from_secs_f32(secs));
        let released = Instant::now();
        handle.send(Input::RecordStop).unwrap();
        let (drafts, entry) = drain_until_entry_within(&events, Duration::from_secs(300));
        let elapsed = released.elapsed();
        println!("  drafts during recording: {}", drafts.len());
        drop(daemon);
        (elapsed, entry)
    }

    /// Three speech fixtures in a row: a phrase of twelve seconds with pauses between sentences
    #[cfg(test)]
    fn concatenated_fixtures() -> PcmAudio {
        let mut samples = Vec::new();
        for name in ["privet_ru.wav", "hello_en.wav", "secret_ru_en.wav"] {
            let path = format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
            let mut reader = hound::WavReader::open(&path).expect("the fixture is in place");
            samples.extend(
                reader
                    .samples::<i16>()
                    .map(|s| f32::from(s.expect("sample")) / f32::from(i16::MAX)),
            );
            // A pause between sentences: the segmenter needs a boundary
            samples.resize(samples.len() + 16_000, 0.0);
        }
        PcmAudio::new(samples, 16_000)
    }

    #[test]
    fn busy_is_reported_over_the_request_handler_too() {
        let h = harness("реплика");
        let handler: &dyn RequestHandler = &h.handle;
        handler
            .handle(Command::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(1));
        let err = handler
            .handle(Command::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Busy);
        drop(h.daemon);
    }
}
