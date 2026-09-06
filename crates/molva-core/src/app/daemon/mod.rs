// SPDX-License-Identifier: MIT
//! Демон: единственное место, где сходятся хоткеи, IPC, микрофон и конвейер.
//!
//! Устройство простое и намеренно однопоточное по решениям: команды со всех источников попадают
//! в один канал, их разбирает управляющий поток с машиной состояний, а долгая обработка уходит
//! в отдельный рабочий поток и возвращается тем же каналом. Поэтому «занят» — это не гонка, а
//! состояние машины, и запись не может стартовать дважды.

pub mod chunked;
pub mod processor;
pub mod state;

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
use crate::domain::notify::Notifier;
use crate::infra::ipc::RequestHandler;
use crate::infra::platform;
use crate::ipc::protocol::{Command, DaemonState, ErrorCode, Event, IpcError};

pub use chunked::{ChunkAccumulator, ChunkContext, ChunkFeeder, ChunkPrefix, ChunkText};
pub use processor::{ProcessError, Processor, ProcessorConfig, SimpleProcessor};
pub use state::{Action, DiscardReason, Input, Machine, Outcome};

/// Команда управляющему потоку и, при необходимости, канал для ответа.
struct Message {
    input: Input,
    reply: Option<Sender<Outcome>>,
}

/// Всё, что приходит управляющему потоку.
///
/// Тик потоковой обработки не проходит через машину состояний: он ничего не решает, а только снимает
/// свежий звук с микрофона, которым владеет этот же поток.
enum Ctl {
    Message(Message),
    ChunkTick { generation: u64 },
}

/// Задание рабочему потоку.
enum Job {
    /// Кусок ещё идущей записи: распознаётся, пока человек говорит.
    Chunk {
        audio: PcmAudio,
        /// Начало записи: от него считается задержка до первой гипотезы.
        started: Instant,
    },
    /// Реплика кончилась.
    Finish {
        /// Вся реплика: идёт в дело, когда кусков не было или они не сложились.
        audio: PcmAudio,
        /// Хвост после последнего куска.
        tail: Option<PcmAudio>,
        mode: Mode,
        style: Option<String>,
        app: Option<String>,
    },
    /// Запись отменили: накопленные куски забыть.
    Discard,
}

/// Всё, что демону нужно снаружи. Железо приходит трейтами, поэтому демон целиком проверяется
/// фейками.
#[derive(Debug)]
pub struct DaemonParts {
    pub audio: Box<dyn AudioSource>,
    pub processor: Box<dyn Processor>,
    pub notifier: Arc<dyn Notifier>,
    pub clock: Arc<dyn Clock>,
    pub config: Config,
}

/// Общее состояние демона, видимое всем ручкам.
#[derive(Debug)]
struct Shared {
    tx: Mutex<Sender<Ctl>>,
    state: Mutex<DaemonState>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    session_id: Uuid,
    started_at: DateTime<Utc>,
    /// Номер текущей записи: сторожевой таймер длительности не должен обрывать следующую.
    generation: AtomicU64,
}

impl Shared {
    /// Разослать событие подписчикам; отвалившиеся отписываются сами.
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

/// Работающий демон. Пока значение живо, живы и его потоки.
#[derive(Debug)]
pub struct Daemon {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Ручка демона: её раздают серверу IPC и источникам горячих клавиш.
#[derive(Debug, Clone)]
pub struct DaemonHandle {
    shared: Arc<Shared>,
}

impl Daemon {
    /// Запустить демон: управляющий поток, рабочий поток и поток уровней сигнала.
    // Три потока и их замыкания читаются как одно целое: каналы, которые они делят, видны
    // только здесь. Разносить их по функциям стоит вместе с дорожкой потоковой обработки,
    // которая эти же замыкания сейчас и меняет.
    #[allow(clippy::too_many_lines)]
    pub fn spawn(parts: DaemonParts) -> Daemon {
        let DaemonParts {
            mut audio,
            mut processor,
            notifier,
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
        });

        let mut machine = Machine::new(config.hotkeys.clone(), clock.clone());
        let max_duration = Duration::from_secs(u64::from(config.audio.max_duration_secs));
        let mut feeder = ChunkFeeder::new(&config.stt, &config.audio);
        let streaming_preview = config.stt.streaming_preview;

        let mut threads = Vec::new();

        // Управляющий поток: единственный, кто трогает машину состояний и микрофон.
        {
            let shared = shared.clone();
            let notifier = notifier.clone();
            let tx = tx.clone();
            let clock = clock.clone();
            let work_tx = work_tx.clone();
            threads.push(std::thread::spawn(move || {
                let mut pending: Option<(Mode, Option<String>, Option<String>)> = None;
                while let Ok(ctl) = rx.recv() {
                    let message = match ctl {
                        // Тик потоковой обработки: снять свежий звук и отправить дозревшие куски в
                        // распознавание, не дожидаясь отпускания клавиши.
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
                        Ctl::Message(message) => message,
                    };
                    let outcome = machine.on(message.input);
                    for action in &outcome.actions {
                        match action {
                            Action::StartCapture { mode, style } => {
                                let app = platform::active_window_class();
                                match audio.start(Some(level_tx.clone())) {
                                    Ok(()) => {
                                        pending = Some((*mode, style.clone(), app));
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
                                        // Микрофон не открылся — машину нельзя оставить в записи.
                                        machine.on(Input::RecordCancel);
                                        shared.set_state(DaemonState::Idle, None);
                                        shared.publish(&Event::Error {
                                            code: ErrorCode::NoDevice,
                                            message: err.to_string(),
                                            hint: Some("проверьте устройство: molva doctor".into()),
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
                                        shared.set_state(DaemonState::Transcribing, Some(*mode));
                                        // Куски, отданные во время записи; если их не было,
                                        // реплика идёт целиком, как раньше.
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
                                        };
                                        if !sent || work_tx.send(job).is_err() {
                                            machine.on(Input::ProcessingFailed);
                                            shared.set_state(DaemonState::Idle, None);
                                        }
                                    }
                                    Err(err) => {
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
                                // Куски отменённой записи не должны прирасти к следующей реплике.
                                let _ = work_tx.send(Job::Discard);
                                // Микрофон мог и не открыться: отказ здесь ничего не меняет.
                                let _ = audio.stop();
                                shared.set_state(DaemonState::Idle, None);
                                tracing::info!(reason = reason.message(), "запись отброшена");
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

        // Рабочий поток: пока он занят распознаванием, управляющий продолжает отвечать «занят».
        {
            let shared = shared.clone();
            let notifier = notifier.clone();
            let tx = tx.clone();
            threads.push(std::thread::spawn(move || {
                // Куски приходят одной очередью в один поток, поэтому порядок текстов — это
                // порядок кусков; перемешаться они не могут.
                let mut chunks = ChunkAccumulator::default();
                while let Ok(job) = work_rx.recv() {
                    let (audio, tail, mode, style, app) = match job {
                        Job::Discard => {
                            chunks.reset();
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
                        } => (audio, tail, mode, style, app),
                    };
                    // Кусков не было или ни в одном не нашлось речи: реплика идёт целиком.
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
                            tracing::error!(%err, "обработка не удалась");
                            notifier.notify("MolvAI", &err.to_string());
                            shared.publish(&Event::Error {
                                code: error_code_for(&err),
                                message: err.to_string(),
                                hint: Some("подробности: molva doctor".into()),
                            });
                            Input::ProcessingFailed
                        }
                    };
                    let _ = tx.send(Ctl::Message(Message { input, reply: None }));
                }
            }));
        }

        // Поток уровней: индикатор в GUI и предупреждение о немом микрофоне.
        {
            let shared = shared.clone();
            threads.push(std::thread::spawn(move || {
                while let Ok(rms) = level_rx.recv() {
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

    /// Дождаться завершения потоков. Возвращает управление, когда каналы закрыты.
    pub fn join(self) {
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

/// Сторож длительности: обрывает запись, если про неё забыли.
fn spawn_max_duration_guard(tx: Sender<Ctl>, shared: Arc<Shared>, generation: u64, max: Duration) {
    if max.is_zero() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(max);
        // Номер поколения защищает следующую запись: сторож старой её не тронет.
        if shared.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = tx.send(Ctl::Message(Message {
            input: Input::MaxDuration,
            reply: None,
        }));
    });
}

/// Будильник потоковой обработки: четыре раза в секунду просит управляющий поток снять свежий звук.
///
/// Сам звук снимает управляющий поток — он один владеет микрофоном. Будильник умолкает, как только
/// запись сменилась на следующую.
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

/// Распознать очередной кусок и показать подписчикам черновик.
///
/// Кусок, который не распознался, реплику не роняет: его текста просто не будет, а остальные куски
/// и хвост дойдут как обычно.
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
        // Обработчик потоковую обработку не умеет: реплику он получит целиком.
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
        Err(err) => tracing::warn!(%err, "кусок реплики не распознан, остальные не пострадают"),
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
    /// Отправить вход и дождаться решения машины состояний.
    pub fn send(&self, input: Input) -> Result<Outcome, IpcError> {
        let (reply_tx, reply_rx) = channel();
        let message = Ctl::Message(Message {
            input,
            reply: Some(reply_tx),
        });
        {
            let tx = self.shared.tx.lock().map_err(|_| internal("демон занят"))?;
            tx.send(message).map_err(|_| internal("демон остановлен"))?;
        }
        reply_rx.recv().map_err(|_| internal("демон остановлен"))
    }

    /// Отправить вход, не дожидаясь ответа: для источников хоткеев.
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

    /// Подписаться на события демона.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = channel();
        if let Ok(mut subscribers) = self.shared.subscribers.lock() {
            subscribers.push(tx);
        }
        rx
    }

    /// Сводка для `molva status`.
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
            Command::Shutdown => {
                // Ответ должен уйти раньше, чем процесс исчезнет.
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(100));
                    std::process::exit(0);
                });
                Ok(serde_json::json!({ "ok": true }))
            }
            other => Err(IpcError {
                code: ErrorCode::BadRequest,
                message: format!(
                    "команда {} пока не поддержана демоном",
                    command_name(&other)
                ),
                hint: Some("сборка дорожки F: доступны ping, status, record.*, subscribe".into()),
            }),
        }
    }

    fn subscribe(&self, _levels: bool) -> Option<Receiver<Event>> {
        Some(DaemonHandle::subscribe(self))
    }
}

/// Имя команды из протокола — для сообщений об ошибках.
fn command_name(cmd: &Command) -> String {
    serde_json::to_value(cmd)
        .ok()
        .and_then(|v| v.get("cmd").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_else(|| "неизвестная".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::{
        FakeAudioSource, FakeClock, FakeStt, MemJournal, RecordingNotifier,
    };
    use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
    use std::time::Duration;

    /// Инжектор, чей результат виден тесту после того, как он уехал в демон.
    #[derive(Debug, Clone, Default)]
    struct SharedInjector {
        injected: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl TextInjector for SharedInjector {
        fn id(&self) -> &'static str {
            "shared"
        }
        fn available(&self) -> bool {
            true
        }
        fn inject(&mut self, text: &str, _mode: OutputMode) -> Result<InjectReport, InjectError> {
            if self.fail {
                return Err(InjectError::Failed("нет активного окна".into()));
            }
            self.injected.lock().unwrap().push(text.to_string());
            Ok(InjectReport {
                method: "shared".into(),
                attempts: Vec::new(),
            })
        }
    }

    struct Harness {
        daemon: Daemon,
        handle: DaemonHandle,
        clock: Arc<FakeClock>,
        injected: Arc<Mutex<Vec<String>>>,
        notifier: Arc<RecordingNotifier>,
    }

    fn harness(text: &str) -> Harness {
        harness_with(text, false, Config::default())
    }

    fn harness_with(text: &str, fail_inject: bool, config: Config) -> Harness {
        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = Arc::new(FakeClock::at(start));
        let notifier = Arc::new(RecordingNotifier::default());
        let injector = SharedInjector {
            injected: Arc::new(Mutex::new(Vec::new())),
            fail: fail_inject,
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
        let daemon = Daemon::spawn(DaemonParts {
            audio: Box::new(FakeAudioSource::silence(2.0)),
            processor: Box::new(processor),
            notifier: notifier.clone(),
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
        }
    }

    fn wait_for_entry(events: &Receiver<Event>) -> crate::domain::entry::Entry {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Entry { entry }) => return *entry,
                Ok(Event::Error { message, .. }) => panic!("демон сообщил об ошибке: {message}"),
                Ok(_) | Err(_) => {}
            }
        }
        panic!("реплика так и не появилась");
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
            "текст должен дойти до инжектора"
        );
        assert_eq!(entry.words, 2);
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
        // Возврат в Idle делает управляющий поток уже после события: дождёмся его.
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
        let h = harness_with("реплика", true, Config::default());
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
        assert!(entry.error.is_some(), "отказ вставки должен быть в записи");
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
    fn an_unsupported_command_is_answered_with_its_name() {
        let h = harness("реплика");
        let handler: &dyn RequestHandler = &h.handle;
        let err = handler.handle(Command::DevicesList).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.message.contains("devices.list"), "{}", err.message);
        drop(h.daemon);
    }

    /// Демон с полным конвейером и микрофоном, отдающим звук порциями: только так работает
    /// потоковая обработка.
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
        let daemon = Daemon::spawn(DaemonParts {
            // Порции по восемь секунд: два тика микрофона дают куски без ожидания в тесте.
            audio: Box::new(FakeAudioSource::paced(speech(20.0), 8_000)),
            processor: Box::new(pipeline),
            notifier: notifier.clone(),
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
        }
    }

    /// Речь с паузами: полторы секунды тона через секунду тишины.
    fn speech(secs: f32) -> PcmAudio {
        let rate = 16_000usize;
        let mut samples = Vec::new();
        while samples.len() < (secs * rate as f32) as usize {
            samples.extend((0..rate * 3 / 2).map(|i| 0.5 * (i as f32 * 0.3).sin()));
            samples.resize(samples.len() + rate, 0.0);
        }
        PcmAudio::new(samples, rate as u32)
    }

    /// Дождаться реплики, собрав по дороге все черновики.
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
                Ok(Event::Error { message, .. }) => panic!("демон сообщил об ошибке: {message}"),
                Ok(_) | Err(_) => {}
            }
        }
        panic!(
            "реплика так и не появилась, черновиков было {}",
            drafts.len()
        );
    }

    /// Записать реплику и вернуть черновики вместе с итоговой записью.
    fn record(harness: &Harness) -> (Vec<String>, crate::domain::entry::Entry) {
        let events = harness.handle.subscribe();
        harness
            .handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        // Тик потоковой обработки идёт четыре раза в секунду: дадим микрофону отдать порции.
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
            "во время записи не пришло и двух черновиков: обработка снова ждёт отпускания"
        );
        for pair in drafts.windows(2) {
            assert!(
                pair[1].starts_with(&pair[0]),
                "черновик потерял порядок кусков: {:?} → {:?}",
                pair[0],
                pair[1]
            );
        }
        let raw = entry.text_raw.clone().unwrap_or_default();
        let words: Vec<&str> = raw.split_whitespace().collect();
        assert!(words.len() >= 2, "итог собран не из кусков: {raw:?}");
        assert_eq!(
            words,
            responses[..words.len()].to_vec(),
            "куски склеились не в том порядке: {raw:?}"
        );
        assert_eq!(
            drafts.last().map(String::as_str),
            Some(words[..words.len() - 1].join(" ").as_str()),
            "последний черновик — это всё, кроме хвоста"
        );
        assert!(
            entry.latency_ms.first_hypothesis.is_some(),
            "время до первой гипотезы не измерено"
        );
        assert!(
            entry.audio_secs > 15.0,
            "длительность реплики посчитана по хвосту, а не по всей записи: {}",
            entry.audio_secs
        );
        assert_eq!(h.injected.lock().unwrap().len(), 1, "вставка ровно одна");
        drop(h.daemon);
    }

    #[test]
    fn with_chunking_off_the_reply_is_recognised_as_a_whole_after_the_release() {
        let mut config = Config::default();
        config.stt.chunked = false;
        let h = streaming_harness(&["целая реплика", "лишний вызов"], config);

        let (drafts, entry) = record(&h);

        assert!(
            drafts.is_empty(),
            "черновики при выключенной нарезке: {drafts:?}"
        );
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
        // Отмена доходит до рабочего потока тем же каналом, что и куски: дадим ей дойти.
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
            "текст отменённой записи прирос к следующей реплике: {raw:?}"
        );
        drop(h.daemon);
    }

    /// Замер на настоящей модели: сколько человек ждёт после отпускания клавиши.
    ///
    /// Микрофон играет речь в реальном времени (порция за тик), поэтому измеряется ровно то, что
    /// видит пользователь: от `RecordStop` до готовой записи. Прогон занимает минуты, поэтому он
    /// ручной: `MOLVA_TEST_MODEL=<путь> cargo test --release -- --ignored real_model_streaming`.
    #[test]
    #[ignore = "нужна скачанная модель whisper: MOLVA_TEST_MODEL=<путь> --ignored"]
    fn real_model_streaming_shortens_the_wait_after_the_release() {
        let Ok(model) = std::env::var("MOLVA_TEST_MODEL") else {
            panic!("задайте MOLVA_TEST_MODEL=<путь к ggml-*.bin>");
        };
        for chunked in [false, true] {
            let mut config = Config::default();
            config.stt.chunked = chunked;
            let (elapsed, entry) = run_real_reply(&model, config);
            println!(
                "chunked = {chunked}: после отпускания {} мс | stt {} мс | до первой гипотезы \
                 {:?} | реплика {:.1} с\n  {}",
                elapsed.as_millis(),
                entry.latency_ms.stt,
                entry.latency_ms.first_hypothesis,
                entry.audio_secs,
                entry.text_final.as_deref().unwrap_or("")
            );
        }
    }

    /// Одна реплика через демон с настоящей моделью: время от отпускания до готовой записи.
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
            }),
            Box::new(MemJournal::default()),
            clock.clone(),
            PipelineConfig::from_config(&config),
        );
        let daemon = Daemon::spawn(DaemonParts {
            // Порция за тик — это воспроизведение в реальном времени, как с живого микрофона.
            audio: Box::new(FakeAudioSource::paced(audio, chunked::TICK_MS as u32)),
            processor: Box::new(pipeline),
            notifier: Arc::new(RecordingNotifier::default()),
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
        println!("  черновиков во время записи: {}", drafts.len());
        drop(daemon);
        (elapsed, entry)
    }

    /// Три речевые фикстуры подряд: реплика на двенадцать секунд с паузами между фразами.
    #[cfg(test)]
    fn concatenated_fixtures() -> PcmAudio {
        let mut samples = Vec::new();
        for name in ["privet_ru.wav", "hello_en.wav", "secret_ru_en.wav"] {
            let path = format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
            let mut reader = hound::WavReader::open(&path).expect("фикстура на месте");
            samples.extend(
                reader
                    .samples::<i16>()
                    .map(|s| f32::from(s.expect("отсчёт")) / f32::from(i16::MAX)),
            );
            // Пауза между фразами: сегментатору нужна граница.
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
