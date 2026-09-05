// SPDX-License-Identifier: MIT
//! Демон: единственное место, где сходятся хоткеи, IPC, микрофон и конвейер.
//!
//! Устройство простое и намеренно однопоточное по решениям: команды со всех источников попадают
//! в один канал, их разбирает управляющий поток с машиной состояний, а долгая обработка уходит
//! в отдельный рабочий поток и возвращается тем же каналом. Поэтому «занят» — это не гонка, а
//! состояние машины, и запись не может стартовать дважды.

pub mod processor;
pub mod state;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::config::Config;
use crate::domain::audio::{AudioSource, PcmAudio};
use crate::domain::clock::Clock;
use crate::domain::entry::Mode;
use crate::domain::inject::{OutputMode, TextInjector};
use crate::domain::notify::Notifier;
use crate::domain::sound::{CueKind, SoundCue};
use crate::infra::ipc::RequestHandler;
use crate::infra::platform;
use crate::ipc::protocol::{Command, DaemonState, ErrorCode, Event, IpcError};

pub use processor::{ProcessError, Processor, ProcessorConfig, SimpleProcessor};
pub use state::{Action, DiscardReason, Input, Machine, Outcome};

/// Команда управляющему потоку и, при необходимости, канал для ответа.
struct Message {
    input: Input,
    reply: Option<Sender<Outcome>>,
}

/// Задание рабочему потоку.
struct Job {
    audio: PcmAudio,
    mode: Mode,
    style: Option<String>,
    app: Option<String>,
}

/// Всё, что демону нужно снаружи. Железо приходит трейтами, поэтому демон целиком проверяется
/// фейками.
pub struct DaemonParts {
    pub audio: Box<dyn AudioSource>,
    pub processor: Box<dyn Processor>,
    pub notifier: Arc<dyn Notifier>,
    /// Звуковые метки начала и конца записи; тишину делает `NullSoundCue`, а не отсутствие вызова.
    pub sound: Arc<dyn SoundCue>,
    /// Способ вставки для готового текста (`inject.text`): повтор реплики из истории.
    ///
    /// Отдельный от конвейера экземпляр: конвейер занят своей репликой, а повтор из истории
    /// приходит по IPC в любой момент и не должен ждать распознавания.
    pub injector: Option<Box<dyn TextInjector>>,
    pub clock: Arc<dyn Clock>,
    pub config: Config,
}

/// Общее состояние демона, видимое всем ручкам.
struct Shared {
    tx: Mutex<Sender<Message>>,
    state: Mutex<DaemonState>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    session_id: Uuid,
    started_at: DateTime<Utc>,
    /// Номер текущей записи: сторожевой таймер длительности не должен обрывать следующую.
    generation: AtomicU64,
    /// Вставка готового текста по `inject.text`.
    injector: Mutex<Option<Box<dyn TextInjector>>>,
    /// Порог `output.auto_type_max_chars` для разрешения режима `auto`.
    auto_type_max_chars: u32,
}

impl Shared {
    /// Разослать событие подписчикам; отвалившиеся отписываются сами.
    fn publish(&self, event: Event) {
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
            self.publish(Event::State { state, mode });
        }
    }
}

/// Работающий демон. Пока значение живо, живы и его потоки.
pub struct Daemon {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Ручка демона: её раздают серверу IPC и источникам горячих клавиш.
#[derive(Clone)]
pub struct DaemonHandle {
    shared: Arc<Shared>,
}

impl Daemon {
    /// Запустить демон: управляющий поток, рабочий поток и поток уровней сигнала.
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

        let (tx, rx) = channel::<Message>();
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
        });

        let mut machine = Machine::new(config.hotkeys.clone(), clock.clone());
        let max_duration = Duration::from_secs(u64::from(config.audio.max_duration_secs));

        let mut threads = Vec::new();

        // Управляющий поток: единственный, кто трогает машину состояний и микрофон.
        {
            let shared = shared.clone();
            let notifier = notifier.clone();
            let sound = sound.clone();
            let tx = tx.clone();
            threads.push(std::thread::spawn(move || {
                let mut pending: Option<(Mode, Option<String>, Option<String>)> = None;
                while let Ok(message) = rx.recv() {
                    let outcome = machine.on(message.input);
                    for action in &outcome.actions {
                        match action {
                            Action::StartCapture { mode, style } => {
                                let app = platform::active_window_class();
                                match audio.start(Some(level_tx.clone())) {
                                    Ok(()) => {
                                        pending = Some((*mode, style.clone(), app));
                                        // Первый из двух сигналов реплики: микрофон открыт.
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
                                    }
                                    Err(err) => {
                                        // Микрофон не открылся — машину нельзя оставить в записи.
                                        sound.play(CueKind::Error);
                                        machine.on(Input::RecordCancel);
                                        shared.set_state(DaemonState::Idle, None);
                                        shared.publish(Event::Error {
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
                                        // Второй и последний сигнал реплики: микрофон закрыт.
                                        sound.play(CueKind::RecordStop);
                                        shared.set_state(DaemonState::Transcribing, Some(*mode));
                                        let job = Job {
                                            audio: pcm,
                                            mode: *mode,
                                            style: style.clone(),
                                            app,
                                        };
                                        if work_tx.send(job).is_err() {
                                            machine.on(Input::ProcessingFailed);
                                            shared.set_state(DaemonState::Idle, None);
                                        }
                                    }
                                    Err(err) => {
                                        sound.play(CueKind::Error);
                                        machine.on(Input::ProcessingFailed);
                                        shared.set_state(DaemonState::Idle, None);
                                        shared.publish(Event::Error {
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
                                // Микрофон мог и не открыться: отказ здесь ничего не меняет.
                                let _ = audio.stop();
                                // Реплики не будет, поэтому и сигнала конца записи нет: слышно,
                                // что нажатие пропало впустую.
                                sound.play(CueKind::Error);
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
                while let Ok(job) = work_rx.recv() {
                    let result = processor.process(
                        job.audio,
                        job.mode,
                        job.style.as_deref(),
                        job.app.as_deref(),
                    );
                    let input = match result {
                        Ok(entry) => {
                            shared.publish(Event::Entry {
                                entry: Box::new(entry),
                            });
                            Input::ProcessingDone
                        }
                        Err(err) => {
                            tracing::error!(%err, "обработка не удалась");
                            notifier.notify("MolvAI", &err.to_string());
                            shared.publish(Event::Error {
                                code: error_code_for(&err),
                                message: err.to_string(),
                                hint: Some("подробности: molva doctor".into()),
                            });
                            Input::ProcessingFailed
                        }
                    };
                    let _ = tx.send(Message { input, reply: None });
                }
            }));
        }

        // Поток уровней: индикатор в GUI и предупреждение о немом микрофоне.
        {
            let shared = shared.clone();
            threads.push(std::thread::spawn(move || {
                while let Ok(rms) = level_rx.recv() {
                    shared.publish(Event::Level { rms });
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
fn spawn_max_duration_guard(
    tx: Sender<Message>,
    shared: Arc<Shared>,
    generation: u64,
    max: Duration,
) {
    if max.is_zero() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(max);
        // Номер поколения защищает следующую запись: сторож старой её не тронет.
        if shared.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = tx.send(Message {
            input: Input::MaxDuration,
            reply: None,
        });
    });
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
        let message = Message {
            input,
            reply: Some(reply_tx),
        };
        {
            let tx = self.shared.tx.lock().map_err(|_| internal("демон занят"))?;
            tx.send(message).map_err(|_| internal("демон остановлен"))?;
        }
        reply_rx.recv().map_err(|_| internal("демон остановлен"))
    }

    /// Отправить вход, не дожидаясь ответа: для источников хоткеев.
    pub fn send_async(&self, input: Input) {
        if let Ok(tx) = self.shared.tx.lock() {
            let _ = tx.send(Message { input, reply: None });
        }
    }

    pub fn state(&self) -> DaemonState {
        self.shared
            .state
            .lock()
            .map(|s| *s)
            .unwrap_or(DaemonState::Idle)
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

    /// Вставить готовый текст: повтор реплики из истории (`molva history paste`).
    ///
    /// Микрофон и распознавание здесь не участвуют вовсе: текст уже есть, нужен только тот же
    /// способ вставки, что и у обычной реплики, вместе с поправкой на класс активного окна.
    pub fn inject_text(
        &self,
        text: &str,
        mode: Option<OutputMode>,
    ) -> Result<serde_json::Value, IpcError> {
        if text.trim().is_empty() {
            return Err(IpcError {
                code: ErrorCode::BadRequest,
                message: "пустой текст вставлять нечего".into(),
                hint: None,
            });
        }
        let mut guard = self
            .shared
            .injector
            .lock()
            .map_err(|_| internal("способ вставки занят"))?;
        let Some(injector) = guard.as_mut() else {
            return Err(IpcError {
                code: ErrorCode::InjectFailed,
                message: "демон запущен без способа вставки".into(),
                hint: Some("текст можно скопировать: molva history show <id>".into()),
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
                hint: Some("текст остался в буфере обмена, нажмите Ctrl+V".into()),
            }),
        }
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
            Command::InjectText { text, mode } => self.inject_text(&text, mode),
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
        FakeAudioSource, FakeClock, FakeStt, MemJournal, RecordingNotifier, RecordingSoundCue,
    };
    use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
    use std::time::Duration;

    /// Инжектор, чей результат виден тесту после того, как он уехал в демон.
    #[derive(Clone, Default)]
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
        sound: Arc<RecordingSoundCue>,
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
        let sound = Arc::new(RecordingSoundCue::default());
        let daemon = Daemon::spawn(DaemonParts {
            audio: Box::new(FakeAudioSource::silence(2.0)),
            processor: Box::new(processor),
            notifier: notifier.clone(),
            sound: sound.clone(),
            // Повтор из истории вставляется тем же фейком: тест видит и реплики, и повторы.
            injector: Some(Box::new(SharedInjector {
                injected: injected.clone(),
                fail: fail_inject,
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
        }
    }

    fn wait_for_entry(events: &Receiver<Event>) -> crate::domain::entry::Entry {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Entry { entry }) => return *entry,
                Ok(Event::Error { message, .. }) => panic!("демон сообщил об ошибке: {message}"),
                Ok(_) => continue,
                Err(_) => continue,
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
    fn a_reply_is_framed_by_exactly_two_sound_cues() {
        // Критерий AG-05: сигнал начала записи и сигнал конца записи — ровно два на реплику.
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
            "на реплику должно приходиться ровно два сигнала"
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
            "отменённая запись не должна звучать как законченная реплика"
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
        // Возврат в Idle делает управляющий поток уже после события: дождёмся его.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while h.handle.state() != DaemonState::Idle && std::time::Instant::now() < deadline {
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
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::State { state, .. }) => {
                    seen.push(state);
                    break;
                }
                Ok(_) => continue,
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
        // Критерий AM-17: реплику из истории можно вставить заново, не диктуя её снова.
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
        let h = harness_with("реплика", true, Config::default());
        let handler: &dyn RequestHandler = &h.handle;
        let err = handler
            .handle(Command::InjectText {
                text: "собрание переносится".into(),
                mode: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InjectFailed);
        assert!(err.hint.unwrap().contains("буфер"), "нужен следующий шаг");
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
