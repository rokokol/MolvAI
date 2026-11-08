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
use crate::ipc::protocol::{
    Command, DaemonState, ErrorCode, Event, FileReport, FileTranscriptionRequest, IpcError,
    WorkerJob,
};

/// What to do when the microphone gives a digital zero. On macOS a common cause is denied
/// access: the system returns silence, not an error
#[cfg(target_os = "macos")]
const SILENT_MICROPHONE_HINT: &str =
    "check access: System Settings → Privacy & Security → Microphone";
#[cfg(not(target_os = "macos"))]
const SILENT_MICROPHONE_HINT: &str = "check the device: molva devices";

/// How long a client waits for a file transcription
///
/// The wait is bounded, because the worker thread serves the phrase and the file in turn: a stuck
/// recognition or a hung model must end with an answer, not with a client that waits forever. Two
/// hours of audio on a slow processor still fit, and a longer file is worth a second attempt rather
/// than an unbounded hold of the MCP session
const FILE_JOB_TIMEOUT: Duration = Duration::from_secs(600);

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
    /// A file that the worker thread must transcribe with the pipeline it already holds
    FileJob {
        request: FileTranscriptionRequest,
        reply: Sender<Result<FileReport, IpcError>>,
    },
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
        mode: Mode,
        app: Option<String>,
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
    /// A file transcription for the same worker thread: one pipeline, one recognizer, one file
    /// at a time
    File {
        request: FileTranscriptionRequest,
        reply: Sender<Result<FileReport, IpcError>>,
    },
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
    /// What the worker thread is busy with, as the clients see it (`status`, state events):
    /// `Some` also means that the slot is taken, so one file at a time needs no second flag
    job: Mutex<Option<WorkerJob>>,
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

    /// The file slot is free again: the worker finished the file or never took it
    fn release_file_job(&self) {
        let taken = {
            let mut job = self
                .job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            job.take().is_some()
        };
        if taken {
            self.publish_state(None);
        }
    }

    /// Take the worker for a file, or say why the file cannot start now
    ///
    /// Two rules, and both keep one recognizer honest: a phrase in recording or processing comes
    /// first, because a person waits for it, and only one file job may hold the worker. The slot
    /// is taken and reported in one step, so a client never reads `idle` with no job while the
    /// file is already on its way to the worker
    fn take_file_slot(&self, state: DaemonState) -> Result<(), IpcError> {
        if state != DaemonState::Idle {
            return Err(IpcError {
                code: ErrorCode::Busy,
                message: "the daemon is recording or processing a phrase".into(),
                hint: Some("try again when the phrase is delivered".into()),
            });
        }
        {
            let mut job = self
                .job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.is_some() {
                return Err(IpcError {
                    code: ErrorCode::Busy,
                    message: "another file transcription is already running".into(),
                    hint: Some("one recognizer transcribes one file at a time".into()),
                });
            }
            *job = Some(WorkerJob::File);
        }
        self.publish_state(None);
        Ok(())
    }

    /// The job of the worker thread, still set while a phrase recorded during it waits
    fn job(&self) -> Option<WorkerJob> {
        *self
            .job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn state(&self) -> DaemonState {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The current state and job as one event: a client that missed one of the two changes
    /// would show a state that contradicts the work of the worker
    fn publish_state(&self, mode: Option<Mode>) {
        let state = self.state();
        let job = self.job();
        self.publish(&Event::State { state, mode, job });
    }

    fn set_state(&self, state: DaemonState, mode: Option<Mode>) {
        let changed = {
            let mut current = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *current == state {
                false
            } else {
                *current = state;
                true
            }
        };
        if changed {
            self.publish_state(mode);
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
    // streaming processing, which changes these same closures
    #[allow(clippy::too_many_lines)]
    pub fn spawn(parts: DaemonParts) -> Daemon {
        let DaemonParts {
            mut audio,
            mut processor,
            notifier: _notifier,
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
            job: Mutex::new(None),
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
                                    let (mode, app) = pending
                                        .as_ref()
                                        .map_or((Mode::Dictation, None), |(mode, _, app)| {
                                            (*mode, app.clone())
                                        });
                                    let job = Job::Chunk {
                                        audio: chunk.audio,
                                        started,
                                        mode,
                                        app,
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
                        // A file transcription uses the pipeline of the worker thread. The
                        // decision is taken here, in the thread that also decides about the
                        // microphone: so a file job cannot slip in between a key press and the
                        // recording it starts, and the user's phrase never waits for a file
                        Ctl::FileJob { request, reply } => {
                            if let Err(error) = shared.take_file_slot(machine.state()) {
                                let _ = reply.send(Err(error));
                            } else {
                                let job = Job::File {
                                    request,
                                    reply: reply.clone(),
                                };
                                if work_tx.send(job).is_err() {
                                    // No worker, no job: the slot must not stay taken
                                    shared.release_file_job();
                                    let _ = reply.send(Err(internal("daemon stopped")));
                                }
                            }
                            continue;
                        }
                        Ctl::Message(message) => message,
                    };
                    // Start of the count for the guarantee "microphone freed after the phrase".
                    // The count runs from the stop command, that is from the key release, to the
                    // stream close (AG-03)
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
                                        // The hold counts from the open microphone. The stream
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
                                        // Chunks emitted during recording: with none, the whole
                                        // phrase goes to recognition
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
                                                mode: *mode,
                                                app: app.clone(),
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
                                // The microphone need not have opened, so a failure here changes
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
            let tx = tx.clone();
            threads.push(std::thread::spawn(move || {
                // Chunks come in one queue to one thread. The order of the texts is the order of
                // the chunks, so they cannot get mixed up
                let mut chunks = ChunkAccumulator::default();
                while let Ok(job) = work_rx.recv() {
                    let (audio, tail, mode, style, app, stop_after_release_ms) = match job {
                        Job::Discard => {
                            chunks.reset();
                            processor.discard_stream();
                            continue;
                        }
                        Job::Reload(config) => {
                            processor.apply_config(&config);
                            tracing::info!(style = %config.style.default, "pipeline reloaded the settings");
                            continue;
                        }
                        Job::Chunk {
                            audio,
                            started,
                            mode,
                            app,
                        } => {
                            process_chunk(
                                processor.as_mut(),
                                &mut chunks,
                                &shared,
                                &audio,
                                started,
                                ChunkPolicy {
                                    preview: streaming_preview,
                                    mode,
                                },
                                app.as_deref(),
                            );
                            continue;
                        }
                        // A file is not a phrase: it never reaches the state machine, and the
                        // recognizer is the one that already holds the weights
                        Job::File { request, reply } => {
                            let answer = processor.transcribe_file(&request).map_err(|err| {
                                IpcError {
                                    code: error_code_for(&err),
                                    message: err.to_string(),
                                    hint: None,
                                }
                            });
                            // The slot is free before the answer leaves, so the next file can
                            // start at once
                            shared.release_file_job();
                            let _ = reply.send(answer);
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
                            // The entry comes first: clients that wait for the phrase must
                            // not mistake its warning for a failed processing. The window then
                            // gets the reason why the model or the paste did not take part
                            let warning = entry.error.clone().map(|error| {
                                let code = if error.starts_with("LLM:") {
                                    ErrorCode::LlmFailed
                                } else {
                                    ErrorCode::InjectFailed
                                };
                                (code, error)
                            });
                            shared.publish(&Event::Entry {
                                entry: Box::new(entry),
                            });
                            if let Some((code, message)) = warning {
                                shared.publish(&Event::Error {
                                    code,
                                    message,
                                    hint: None,
                                });
                            }
                            Input::ProcessingDone
                        }
                        Err(err) => {
                            tracing::error!(%err, "processing failed");
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
        // watcher lives here and not in the cpal callback. A real-time callback cannot show a
        // notification, and from here the error goes by the same path as a recording error
        {
            let shared = shared.clone();
            let clock = clock.clone();
            threads.push(std::thread::spawn(move || {
                let mut silence = ZeroLevelWatch::with_defaults();
                while let Ok(rms) = level_rx.recv() {
                    if let Some(message) = silence.observe(rms, clock.instant()) {
                        tracing::warn!("{message}");
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

/// What to do with a recognized chunk: show the draft, type it, or both
#[derive(Debug, Clone, Copy)]
struct ChunkPolicy {
    preview: bool,
    mode: Mode,
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
    policy: ChunkPolicy,
    app: Option<&str>,
) {
    let context = chunks.context();
    let Some(result) = processor.transcribe_chunk(audio, &context) else {
        // The processor cannot do streaming processing: it will get the phrase whole
        return;
    };
    match result {
        Ok(chunk) => {
            let since_start = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            let mut typed = String::new();
            if policy.mode == Mode::Dictation {
                match processor.stream_chunk(&chunk, app) {
                    Some(Ok(progress)) => typed = progress.typed,
                    Some(Err(err)) => {
                        tracing::warn!(
                            %err,
                            "chunk not typed, the phrase continues without streaming"
                        );
                    }
                    None => {}
                }
            }
            chunks.push(chunk, since_start);
            if policy.preview {
                // The draft is what the field holds: the typed chunks, or the recognized text
                // when nothing was typed yet
                let text = if typed.is_empty() {
                    chunks.draft()
                } else {
                    typed
                };
                shared.publish(&Event::Hypothesis { text });
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
        ProcessError::Journal(_) | ProcessError::FilesUnsupported => ErrorCode::Internal,
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
        self.shared.state()
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

    /// Transcribe one file with the recognizer and the pipeline that this daemon already holds
    ///
    /// The job goes to the worker thread, which owns the only pipeline of the process: so the
    /// daemon never loads a second model for a file. The wait is bounded by [`FILE_JOB_TIMEOUT`]:
    /// a stuck recognition must end with an answer and not with a client that waits forever
    pub fn transcribe_file(
        &self,
        request: FileTranscriptionRequest,
    ) -> Result<FileReport, IpcError> {
        let (reply, answer) = channel();
        {
            let tx = self.shared.tx.lock().map_err(|_| internal("daemon busy"))?;
            tx.send(Ctl::FileJob { request, reply })
                .map_err(|_| internal("daemon stopped"))?;
        }
        match answer.recv_timeout(FILE_JOB_TIMEOUT) {
            Ok(answer) => answer,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(IpcError {
                code: ErrorCode::Internal,
                message: format!(
                    "the file transcription did not finish in {} min",
                    FILE_JOB_TIMEOUT.as_secs() / 60
                ),
                hint: Some("the daemon log says what it is doing; try a shorter file".into()),
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(internal("daemon stopped")),
        }
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

    /// Summary for `molva status` and the window: the state machine and the job of the worker
    /// are two facts, and a client needs both
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state(),
            "job": self.shared.job(),
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
            Command::FileTranscribe(request) => {
                let report = self.transcribe_file(request)?;
                serde_json::to_value(report).map_err(|err| internal(&err.to_string()))
            }
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
    use crate::domain::llm::LlmClient;
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
        assert!(
            h.notifier.messages.lock().unwrap().is_empty(),
            "the window owns the notice; the daemon must not send a system toast"
        );
        drop(h.daemon);
    }

    #[test]
    fn a_short_tap_through_ipc_latches_the_recording_instead_of_finishing_it() {
        // The compositor bind sends `record start` on press and `record stop` on release. A short
        // tap arrives as two requests in a row. It must turn on hands-free rather than return an
        // empty phrase
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
        // Criterion AG-01/AG-03: outside a recording the microphone is not busy. The time of its
        // release after the key release is visible in the journal, so it is a measurement and
        // not a claim
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
        assert!(h.notifier.messages.lock().unwrap().is_empty());
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
        streaming_harness_with(responses, config, None)
    }

    /// The same, with a model that translates the phrase after the release
    fn streaming_harness_with(
        responses: &[&str],
        config: Config,
        llm: Option<Arc<dyn LlmClient>>,
    ) -> Harness {
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
            llm,
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
        let mut config = Config::default();
        // This test checks the whole-phrase paste of a chunked reply; the streaming paste has
        // its own test below
        config.stt.streaming_inject = false;
        let h = streaming_harness(&responses, config);

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
    fn chunks_are_typed_while_the_recording_runs() {
        let responses = ["раз", "два", "три", "четыре", "пять"];
        let mut config = Config::default();
        // The words are checked literally, not after the number rule turns them into digits
        config.rules.numbers_as_digits = false;
        config.stt.streaming_inject = true;
        let h = streaming_harness(&responses, config);

        let (_drafts, entry) = record(&h);

        let typed = h.injected.lock().unwrap().clone();
        assert!(
            typed.len() >= 2,
            "the chunks did not reach the field during the recording: {typed:?}"
        );
        assert_eq!(
            entry.inject_method.as_deref(),
            Some("streamed"),
            "the journal does not say the phrase was typed in chunks"
        );
        let typed_text = typed.concat();
        assert!(
            typed_text.split_whitespace().count() >= 2,
            "the field got one word instead of a phrase: {typed_text:?}"
        );
        let final_text = entry.text_final.clone().unwrap_or_default();
        assert!(
            final_text.starts_with(typed_text.trim()),
            "the journal text does not start with what was typed: {final_text:?} against {typed_text:?}"
        );
        let lower = |text: &str| {
            text.split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            lower(&final_text),
            lower(entry.text_raw.as_deref().unwrap_or_default()),
            "the streamed phrase lost a tail word: {final_text:?} against {:?}",
            entry.text_raw
        );
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

    // --- File jobs on the one worker thread ---

    /// A processor that serves a file and holds it until the test lets it finish
    ///
    /// The log is the order of the work of the worker thread: the only way to see that a phrase
    /// recorded during a file job was queued and not lost
    #[derive(Debug)]
    struct FileJobProbe {
        inner: SimpleProcessor<SharedInjector, MemJournal>,
        log: Arc<Mutex<Vec<String>>>,
        started: Arc<Mutex<bool>>,
        release: Arc<Mutex<bool>>,
    }

    impl Processor for FileJobProbe {
        fn process(
            &mut self,
            audio: PcmAudio,
            mode: Mode,
            style: Option<&str>,
            app_hint: Option<&str>,
        ) -> Result<crate::domain::entry::Entry, ProcessError> {
            let entry = self.inner.process(audio, mode, style, app_hint)?;
            self.log.lock().unwrap().push(format!(
                "phrase:{}",
                entry.text_final.clone().unwrap_or_default()
            ));
            Ok(entry)
        }

        fn transcribe_file(
            &mut self,
            request: &FileTranscriptionRequest,
        ) -> Result<FileReport, ProcessError> {
            self.log.lock().unwrap().push("file:start".into());
            *self.started.lock().unwrap() = true;
            wait_until(
                || *self.release.lock().unwrap(),
                "the file job was never released",
            );
            self.log.lock().unwrap().push("file:done".into());
            Ok(FileReport {
                results: vec![crate::app::file_result::FileResult {
                    file: request.path.display().to_string(),
                    text: "текст файла".into(),
                    segments: Vec::new(),
                    audio_secs: 1.0,
                    latency_ms: 3,
                    language: None,
                }],
                errors: Vec::new(),
            })
        }
    }

    /// Poll a flag that another thread sets: the daemon threads have no other place to report
    fn wait_until(mut condition: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out: {what}");
    }

    struct FileProbeHarness {
        daemon: Daemon,
        handle: DaemonHandle,
        clock: Arc<FakeClock>,
        log: Arc<Mutex<Vec<String>>>,
        release: Arc<Mutex<bool>>,
    }

    fn file_probe_harness(text: &str) -> FileProbeHarness {
        let config = Config::default();
        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = Arc::new(FakeClock::at(start));
        let notifier = Arc::new(RecordingNotifier::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(Mutex::new(false));
        let release = Arc::new(Mutex::new(false));
        let probe = FileJobProbe {
            inner: SimpleProcessor::new(
                Box::new(FakeStt::returning(text)),
                SharedInjector {
                    injected: Arc::new(Mutex::new(Vec::new())),
                    fail: false,
                    delay: Duration::ZERO,
                },
                MemJournal::default(),
                clock.clone(),
                notifier.clone(),
                ProcessorConfig::from_config(&config, Uuid::nil()),
            ),
            log: log.clone(),
            started: started.clone(),
            release: release.clone(),
        };
        let daemon = Daemon::spawn(DaemonParts {
            audio: Box::new(SharedAudio(Arc::new(Mutex::new(FakeAudioSource::silence(
                2.0,
            ))))),
            processor: Box::new(probe),
            notifier,
            sound: Arc::new(RecordingSoundCue::default()),
            injector: None,
            clock: clock.clone(),
            config,
        });
        let handle = daemon.handle();
        FileProbeHarness {
            daemon,
            handle,
            clock,
            log,
            release,
        }
    }

    fn file_request(path: &str) -> FileTranscriptionRequest {
        FileTranscriptionRequest {
            path: PathBuf::from(path),
            style: None,
            timecodes: true,
            no_llm: false,
            source: crate::domain::entry::Source::Mcp,
        }
    }

    fn status_job(handle: &DaemonHandle) -> serde_json::Value {
        <DaemonHandle as RequestHandler>::handle(handle, Command::Status).unwrap()["job"].clone()
    }

    fn start_file_job(handle: &DaemonHandle) -> Receiver<Result<FileReport, IpcError>> {
        let (tx, rx) = channel();
        let handle = handle.clone();
        std::thread::spawn(move || {
            let _ = tx.send(handle.transcribe_file(file_request("/tmp/lecture.wav")));
        });
        rx
    }

    #[test]
    fn a_file_job_is_visible_in_the_status_and_in_the_state_event() {
        let h = file_probe_harness("реплика");
        let events = h.handle.subscribe();
        let answer = start_file_job(&h.handle);
        wait_until(
            || h.log.lock().unwrap().first().map(String::as_str) == Some("file:start"),
            "the file job never started",
        );

        // The state machine is idle, and the status says what the worker is really doing
        let handler: &dyn RequestHandler = &h.handle;
        let status = handler.handle(Command::Status).unwrap();
        assert_eq!(status["state"], "idle");
        assert_eq!(status["job"], "file");

        let mut saw_file_job = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::State {
                    job: Some(WorkerJob::File),
                    ..
                }) => {
                    saw_file_job = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(saw_file_job, "no state event carried the file job");

        *h.release.lock().unwrap() = true;
        let report = answer
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(report.results[0].text, "текст файла");
        // The slot is free again before the client gets its answer
        assert_eq!(status_job(&h.handle), serde_json::Value::Null);
        drop(h.daemon);
    }

    #[test]
    fn a_phrase_recorded_during_a_file_job_waits_for_it_and_is_processed_after() {
        // The recording must not be refused while the file job holds the worker: the audio is
        // the one thing that cannot be repeated, the file can wait for it
        let h = file_probe_harness("реплика после файла");
        let events = h.handle.subscribe();
        let answer = start_file_job(&h.handle);
        wait_until(
            || h.log.lock().unwrap().first().map(String::as_str) == Some("file:start"),
            "the file job never started",
        );

        let outcome = h
            .handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert!(outcome.is_ok(), "the recording was refused: {outcome:?}");
        assert_eq!(h.handle.state(), DaemonState::Recording);

        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        // The phrase is in the queue of the worker, behind the file that is still running
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            *h.log.lock().unwrap(),
            vec!["file:start".to_string()],
            "the phrase was processed before the file finished"
        );

        *h.release.lock().unwrap() = true;
        let report = answer
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(report.ok());
        let entry = wait_for_entry(&events);
        assert_eq!(entry.text_final.as_deref(), Some("реплика после файла"));
        assert_eq!(
            *h.log.lock().unwrap(),
            vec![
                "file:start".to_string(),
                "file:done".to_string(),
                "phrase:реплика после файла".to_string()
            ],
            "the phrase did not run right after the file"
        );
        drop(h.daemon);
    }

    #[test]
    fn a_file_job_is_refused_while_a_phrase_records() {
        // The file must not take the recognizer away from a person who is speaking
        let h = file_probe_harness("реплика");
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        h.clock.advance(Duration::from_secs(2));
        let err = h
            .handle
            .transcribe_file(file_request("/tmp/lecture.wav"))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Busy);
        assert!(err.hint.is_some(), "the refusal must say what to do next");
        assert!(
            h.log.lock().unwrap().is_empty(),
            "the refused file reached the worker"
        );
        h.handle.send(Input::RecordCancel).unwrap();
        drop(h.daemon);
    }

    #[test]
    fn a_second_file_job_is_refused_while_the_first_one_runs() {
        let h = file_probe_harness("реплика");
        let first = start_file_job(&h.handle);
        wait_until(
            || h.log.lock().unwrap().first().map(String::as_str) == Some("file:start"),
            "the file job never started",
        );
        let err = h
            .handle
            .transcribe_file(file_request("/tmp/second.wav"))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Busy);

        *h.release.lock().unwrap() = true;
        first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        // The slot is free: the next file is accepted
        let second = start_file_job(&h.handle);
        wait_until(
            || {
                h.log
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|line| *line == "file:start")
                    .count()
                    == 2
            },
            "the second file job never started",
        );
        *h.release.lock().unwrap() = true;
        second
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        drop(h.daemon);
    }
}
