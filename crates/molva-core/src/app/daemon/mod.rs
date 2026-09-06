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
    ChunkTick {
        generation: u64,
    },
    /// Перечитанные настройки: хоткеи для машины состояний, остальное — рабочему потоку.
    Reload(Box<Config>),
}

/// Задание рабочему потоку.
enum Job {
    /// Перечитанные настройки для конвейера: стиль по умолчанию, правила, пороги.
    Reload(Box<Config>),
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
        /// От отпускания клавиши до фактического закрытия потока микрофона, миллисекунды.
        stop_after_release_ms: u32,
    },
    /// Запись отменили: накопленные куски забыть.
    Discard,
}

/// Миллисекунды между двумя точками монотонного времени, без переполнения.
fn millis_between(from: Instant, to: Instant) -> u32 {
    u32::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u32::MAX)
}

/// Всё, что демону нужно снаружи. Железо приходит трейтами, поэтому демон целиком проверяется
/// фейками.
#[derive(Debug)]
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
#[derive(Debug)]
struct Shared {
    tx: Mutex<Sender<Ctl>>,
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
    /// Файл настроек для `config.reload`; `None` — путь по умолчанию.
    config_path: Mutex<Option<PathBuf>>,
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

        // Управляющий поток: единственный, кто трогает машину состояний и микрофон.
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
                        Ctl::Reload(config) => {
                            machine.set_hotkeys(config.hotkeys.clone());
                            let _ = work_tx.send(Job::Reload(config));
                            shared.publish(&Event::ConfigReloaded);
                            continue;
                        }
                        Ctl::Message(message) => message,
                    };
                    // Отсчёт гарантии «микрофон освобождён после реплики»: от команды остановки
                    // (то есть от отпускания клавиши) до закрытия потока (AG-03).
                    let released_from = control_clock.instant();
                    let input_label = format!("{:?}", message.input);
                    let outcome = machine.on(message.input);
                    tracing::debug!(
                        input = %input_label,
                        actions = ?outcome.actions,
                        state = ?machine.state(),
                        in_flight = machine.in_flight(),
                        "машина состояний"
                    );
                    for action in &outcome.actions {
                        match action {
                            Action::StartCapture { mode, style } => {
                                let app = platform::active_window_class();
                                match audio.start(Some(level_tx.clone())) {
                                    Ok(()) => {
                                        // Удержание считается от открытого микрофона: сам старт
                                        // потока занимает до 300 мс, и короткий тап иначе выглядел
                                        // бы длинным удержанием.
                                        machine.capture_started(control_clock.instant());
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
                                        sound.play(CueKind::Error);
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
                                        // Второй и последний сигнал реплики: микрофон закрыт.
                                        sound.play(CueKind::RecordStop);
                                        let stop_after_release_ms =
                                            millis_between(released_from, control_clock.instant());
                                        tracing::info!(
                                            ms = stop_after_release_ms,
                                            "микрофон освобождён"
                                        );
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
                                // Куски отменённой записи не должны прирасти к следующей реплике.
                                let _ = work_tx.send(Job::Discard);
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
                // Куски приходят одной очередью в один поток, поэтому порядок текстов — это
                // порядок кусков; перемешаться они не могут.
                let mut chunks = ChunkAccumulator::default();
                while let Ok(job) = work_rx.recv() {
                    let (audio, tail, mode, style, app, stop_after_release_ms) = match job {
                        Job::Discard => {
                            chunks.reset();
                            continue;
                        }
                        Job::Reload(config) => {
                            processor.apply_config(&config);
                            tracing::info!(style = %config.style.default, "конвейер перечитал настройки");
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
                    // Гарантия «микрофон освобождён после реплики» должна быть видна в журнале,
                    // а не только в логе демона.
                    processor.set_stop_after_release(stop_after_release_ms);
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
    /// Откуда демон перечитывает настройки по `config.reload`; без пути — файл по умолчанию.
    pub fn set_config_path(&self, path: PathBuf) {
        if let Ok(mut slot) = self.shared.config_path.lock() {
            *slot = Some(path);
        }
    }

    /// Перечитать файл настроек и разослать его машине состояний и конвейеру.
    /// `style` подменяет стиль по умолчанию: так работает `style.set` из трея и CLI.
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
            message: format!("настройки не перечитаны: {e}"),
            hint: Some("проверьте файл: molva config validate".into()),
        })?;
        if let Some(style) = style {
            config.style.default = style;
        }
        let tx = self.shared.tx.lock().map_err(|_| internal("демон занят"))?;
        tx.send(Ctl::Reload(Box::new(config.clone())))
            .map_err(|_| internal("демон остановлен"))?;
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
    #[derive(Debug, Clone, Default)]
    struct SharedInjector {
        injected: Arc<Mutex<Vec<String>>>,
        fail: bool,
        /// Задержка вставки: так тест держит демон в обработке, пока шлёт новые нажатия.
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
                return Err(InjectError::Failed("нет активного окна".into()));
            }
            self.injected.lock().unwrap().push(text.to_string());
            Ok(InjectReport {
                method: "shared".into(),
                attempts: Vec::new(),
            })
        }
    }

    /// Микрофон, за которым тест продолжает следить после того, как отдал его демону.
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
        /// Потоковая обработка снимает свежий звук через этот метод: обёртка обязана его передать,
        /// иначе куски во время записи просто не появятся.
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
            // Повтор из истории вставляется тем же фейком: тест видит и реплики, и повторы.
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
    fn a_short_tap_through_ipc_latches_the_recording_instead_of_finishing_it() {
        // Бинд композитора шлёт `record start` на нажатие и `record stop` на отпускание:
        // короткий тап приходит как два запроса подряд и обязан включить hands-free,
        // а не отдать пустую реплику.
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
            "после короткого тапа запись продолжается без клавиши"
        );

        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        let entry = wait_for_entry(&events);
        assert_eq!(entry.text_final.as_deref(), Some("тестовая реплика"));
        drop(h.daemon);
    }

    #[test]
    fn a_press_during_processing_starts_the_next_recording_and_both_replies_arrive_in_order() {
        // Вставка держит демон в обработке 400 мс; следующая реплика пишется, не дожидаясь её.
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
            "запись следующей реплики идёт, пока первая обрабатывается"
        );
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();

        let first = wait_for_entry(&events);
        let second = wait_for_entry(&events);
        assert_eq!(first.text_final.as_deref(), Some("тестовая реплика"));
        assert_eq!(second.text_final.as_deref(), Some("тестовая реплика"));
        assert!(first.ts <= second.ts, "реплики обрабатываются по порядку");
        assert_eq!(
            h.injected.lock().unwrap().len(),
            2,
            "обе реплики дошли до вставки"
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
        assert!(saw_reload, "подписчики узнают о перечитанных настройках");

        // Битый файл — ошибка с подсказкой, а не молчаливое «перечитано».
        std::fs::write(&config_path, "[style\n").unwrap();
        let err = h.handle.handle(Command::ConfigReload).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.hint.is_some());
        drop(h.daemon);
    }

    #[test]
    fn the_microphone_is_free_outside_a_recording_and_the_delay_lands_in_the_entry() {
        // Критерий AG-01/AG-03: вне записи микрофон не занят, а время его освобождения после
        // отпускания клавиши видно в журнале, а не только на слово разработчика.
        let h = harness("реплика");
        assert!(!h.audio.is_recording(), "до записи микрофон свободен");
        let events = h.handle.subscribe();
        h.handle
            .send(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            })
            .unwrap();
        assert!(h.audio.is_recording(), "во время записи микрофон занят");
        h.clock.advance(Duration::from_secs(2));
        h.handle.send(Input::RecordStop).unwrap();
        let entry = wait_for_entry(&events);
        assert!(
            !h.audio.is_recording(),
            "после реплики микрофон обязан быть свободен"
        );
        let released = entry
            .latency_ms
            .stop_after_release
            .expect("замер освобождения микрофона должен попасть в запись");
        assert!(
            released < 500,
            "микрофон освобождается сразу: {released} мс"
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
        let h = harness_with("реплика", true, Config::default(), Duration::ZERO);
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
        // Порции по восемь секунд: два тика микрофона дают куски без ожидания в тесте.
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
                delay: Duration::ZERO,
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
            // Замер идёт с настоящей моделью: звук и вставка тут ни при чём.
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
