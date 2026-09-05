// SPDX-License-Identifier: MIT
//! Транспорт IPC: локальный сокет, по строке JSON на сообщение.
//!
//! Формат специально примитивный: одна строка — один `Request` или один `Response`/`Event`.
//! Так к демону можно достучаться из `socat` и из скрипта на любом языке, а не только из CLI.
//!
//! Путь к сокету всегда приходит параметром: демон, CLI и тесты обязаны договариваться явно,
//! иначе тест на сокете во временном каталоге начинает зависеть от переменных окружения.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{
    GenericFilePath, ListenerOptions, RecvHalf, SendHalf, Stream, ToFsName,
};
use serde_json::Value;
use thiserror::Error;

use crate::ipc::protocol::{Command, ErrorCode, Event, IpcError, Request, Response};

#[derive(Debug, Error)]
pub enum IpcServerError {
    #[error("не удалось занять сокет {path}: {message}")]
    Bind { path: PathBuf, message: String },
    #[error("ошибка сокета: {0}")]
    Io(String),
}

#[derive(Debug, Error)]
pub enum IpcClientError {
    #[error("демон недоступен ({path}): {message}")]
    NotRunning { path: PathBuf, message: String },
    #[error("обрыв связи с демоном: {0}")]
    Io(String),
    #[error("демон ответил ошибкой: {}", .0.message)]
    Daemon(IpcError),
    #[error("непонятный ответ демона: {0}")]
    Protocol(String),
}

impl IpcClientError {
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            IpcClientError::Daemon(err) => Some(err.code),
            _ => None,
        }
    }
}

/// Путь к сокету по умолчанию.
///
/// На Linux это `$XDG_RUNTIME_DIR/molva.sock`: каталог чистится при выходе из сессии, поэтому
/// мёртвых сокетов там не остаётся. Если каталога нет — `/tmp/molva-<uid>.sock`, отдельный для
/// каждого пользователя, иначе на общей машине двое дерутся за один файл.
pub fn socket_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"\\.\pipe\molva")
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        dir.join("molva.sock")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            let dir = PathBuf::from(dir);
            if dir.is_dir() {
                return dir.join("molva.sock");
            }
        }
        PathBuf::from(format!("/tmp/molva-{}.sock", current_uid()))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn current_uid() -> String {
    use std::os::unix::fs::MetadataExt;
    if let Ok(meta) = std::fs::metadata("/proc/self") {
        return meta.uid().to_string();
    }
    std::env::var("USER").unwrap_or_else(|_| "default".into())
}

fn name_for(path: &Path) -> Result<interprocess::local_socket::Name<'static>, std::io::Error> {
    #[cfg(windows)]
    {
        // Windows принимает только имена каналов `\\.\pipe\…`. Любой другой путь (например,
        // временный каталог в тестах или `--socket /tmp/x.sock` из чужой инструкции) отображается
        // на имя канала детерминированно, чтобы клиент и сервер сошлись на одном и том же.
        let text = path.to_string_lossy();
        if !text.starts_with(r"\\.\pipe\") {
            let sanitized: String = text
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            return PathBuf::from(format!(r"\\.\pipe\molva-{sanitized}"))
                .to_fs_name::<GenericFilePath>();
        }
    }
    path.to_path_buf().to_fs_name::<GenericFilePath>()
}

/// Обработчик запросов на стороне демона.
pub trait RequestHandler: Send + Sync + 'static {
    /// Ответ на команду. `Ok` кладётся в `result`, `Err` — в `error`.
    fn handle(&self, cmd: Command) -> Result<Value, IpcError>;
    /// Канал событий для подписчика; `None` — подписка не поддерживается.
    fn subscribe(&self, levels: bool) -> Option<Receiver<Event>>;
}

/// Сервер локального сокета: поток на соединение.
#[derive(Debug)]
pub struct Server {
    listener: interprocess::local_socket::Listener,
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Занять сокет. Мёртвый файл сокета удаляется: иначе демон не поднимется после сбоя.
    pub fn bind(path: &Path) -> Result<Self, IpcServerError> {
        if path_is_stale(path) {
            let _ = std::fs::remove_file(path);
        }
        let name = name_for(path).map_err(|e| IpcServerError::Bind {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .map_err(|e| IpcServerError::Bind {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Флаг остановки: выставить и постучаться в сокет, чтобы `accept` разблокировался.
    pub fn stopper(&self) -> Stopper {
        Stopper {
            stop: self.stop.clone(),
            path: self.path.clone(),
        }
    }

    /// Принимать соединения, пока не попросят остановиться.
    pub fn serve(self, handler: Arc<dyn RequestHandler>) -> Result<(), IpcServerError> {
        for incoming in self.listener.incoming() {
            if self.stop.load(Ordering::SeqCst) {
                break;
            }
            match incoming {
                Ok(stream) => {
                    let handler = handler.clone();
                    // Поток на соединение: подписчик держит своё соединение часами, и он не
                    // должен мешать `molva status` получить ответ.
                    std::thread::spawn(move || {
                        if let Err(err) = serve_connection(stream, handler) {
                            tracing::debug!(%err, "соединение закрыто");
                        }
                    });
                }
                Err(err) => tracing::warn!(%err, "не удалось принять соединение"),
            }
        }
        let _ = std::fs::remove_file(&self.path);
        Ok(())
    }
}

/// Ручка остановки сервера.
#[derive(Debug)]
pub struct Stopper {
    stop: Arc<AtomicBool>,
    path: PathBuf,
}

impl Stopper {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // Разбудить `accept`: без соединения он будет ждать вечно.
        if let Ok(name) = name_for(&self.path) {
            let _ = Stream::connect(name);
        }
    }
}

fn serve_connection(
    stream: Stream,
    handler: Arc<dyn RequestHandler>,
) -> Result<(), IpcServerError> {
    let (recv, mut send) = stream.split();
    let mut reader = BufReader::new(recv);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| IpcServerError::Io(e.to_string()))?;
        if read == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                // id неизвестен — отвечаем нулевым, чтобы клиент увидел причину, а не тишину.
                write_line(
                    &mut send,
                    &Response::err(0, ErrorCode::BadRequest, err.to_string(), None),
                )?;
                continue;
            }
        };
        let id = request.id;
        if let Command::Subscribe { levels } = request.cmd {
            let Some(events) = handler.subscribe(levels) else {
                write_line(
                    &mut send,
                    &Response::err(id, ErrorCode::BadRequest, "подписка недоступна", None),
                )?;
                continue;
            };
            write_line(&mut send, &Response::ok(id, Value::Null))?;
            return stream_events(&mut send, events);
        }
        let response = match handler.handle(request.cmd) {
            Ok(result) => Response::ok(id, result),
            Err(err) => Response {
                id,
                ok: false,
                result: None,
                error: Some(err),
            },
        };
        write_line(&mut send, &response)?;
    }
}

fn stream_events(send: &mut SendHalf, events: Receiver<Event>) -> Result<(), IpcServerError> {
    for event in events {
        write_line(send, &event)?;
    }
    Ok(())
}

fn write_line<T: serde::Serialize>(send: &mut SendHalf, value: &T) -> Result<(), IpcServerError> {
    let mut line = serde_json::to_string(value).map_err(|e| IpcServerError::Io(e.to_string()))?;
    line.push('\n');
    send.write_all(line.as_bytes())
        .map_err(|e| IpcServerError::Io(e.to_string()))?;
    send.flush().map_err(|e| IpcServerError::Io(e.to_string()))
}

/// Есть ли файл сокета, к которому уже никто не слушает.
fn path_is_stale(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match name_for(path) {
        Ok(name) => Stream::connect(name).is_err(),
        Err(_) => false,
    }
}

/// Клиент локального сокета.
pub struct Client {
    reader: BufReader<RecvHalf>,
    writer: SendHalf,
    next_id: u64,
    path: PathBuf,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("path", &self.path).finish()
    }
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self, IpcClientError> {
        let name = name_for(path).map_err(|e| IpcClientError::NotRunning {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let stream = Stream::connect(name).map_err(|e| IpcClientError::NotRunning {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let (recv, writer) = stream.split();
        Ok(Self {
            reader: BufReader::new(recv),
            writer,
            next_id: 1,
            path: path.to_path_buf(),
        })
    }

    /// Отправить команду и дождаться ответа.
    pub fn call(&mut self, cmd: Command) -> Result<Response, IpcClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_string(&Request::new(id, cmd))
            .map_err(|e| IpcClientError::Protocol(e.to_string()))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        self.writer
            .flush()
            .map_err(|e| IpcClientError::Io(e.to_string()))?;

        let mut answer = String::new();
        let read = self
            .reader
            .read_line(&mut answer)
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        if read == 0 {
            return Err(IpcClientError::Io(format!(
                "демон закрыл соединение ({})",
                self.path.display()
            )));
        }
        serde_json::from_str(&answer).map_err(|e| IpcClientError::Protocol(e.to_string()))
    }

    /// Как `call`, но ошибка демона становится ошибкой Rust.
    pub fn call_ok(&mut self, cmd: Command) -> Result<Value, IpcClientError> {
        let response = self.call(cmd)?;
        if let Some(err) = response.error {
            return Err(IpcClientError::Daemon(err));
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    /// Подписаться на события: соединение уходит под поток событий целиком.
    pub fn subscribe(mut self, levels: bool) -> Result<Events, IpcClientError> {
        let response = self.call(Command::Subscribe { levels })?;
        if let Some(err) = response.error {
            return Err(IpcClientError::Daemon(err));
        }
        Ok(Events {
            reader: self.reader,
        })
    }
}

/// Поток событий демона.
#[derive(Debug)]
pub struct Events {
    reader: BufReader<RecvHalf>,
}

impl Iterator for Events {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(event) => return Some(event),
                // Неизвестное событие — не повод рвать подписку: протокол растёт аддитивно.
                Err(err) => tracing::debug!(%err, "неизвестное событие пропущено"),
            }
        }
    }
}

/// Жив ли демон на этом сокете. Возвращает pid из ответа `ping`, если он его сообщил.
pub fn ping(path: &Path) -> Option<u32> {
    let mut client = Client::connect(path).ok()?;
    let response = client.call(Command::Ping).ok()?;
    if !response.ok {
        return None;
    }
    response
        .result
        .as_ref()
        .and_then(|v| v.get("pid"))
        .and_then(Value::as_u64)
        .map(|pid| pid as u32)
        .or(Some(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::Mode;
    use std::sync::mpsc::channel;
    use std::sync::Mutex;
    use std::time::Duration;

    struct EchoHandler {
        seen: Mutex<Vec<Command>>,
        events: Mutex<Option<Receiver<Event>>>,
    }

    impl EchoHandler {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                seen: Mutex::new(Vec::new()),
                events: Mutex::new(None),
            })
        }
    }

    impl RequestHandler for EchoHandler {
        fn handle(&self, cmd: Command) -> Result<Value, IpcError> {
            self.seen.lock().unwrap().push(cmd.clone());
            match cmd {
                Command::Ping => Ok(serde_json::json!({ "pid": 4242 })),
                Command::RecordStart { mode, .. } => {
                    if matches!(mode, Mode::Command) {
                        return Err(IpcError {
                            code: ErrorCode::Busy,
                            message: "занят".into(),
                            hint: None,
                        });
                    }
                    Ok(serde_json::json!({ "started": true }))
                }
                _ => Ok(Value::Null),
            }
        }

        fn subscribe(&self, _levels: bool) -> Option<Receiver<Event>> {
            self.events.lock().unwrap().take()
        }
    }

    /// Сокет живёт во временном каталоге: тесты не трогают ни `$XDG_RUNTIME_DIR`, ни `/tmp`.
    fn started(handler: Arc<dyn RequestHandler>) -> (PathBuf, tempfile::TempDir, Stopper) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("molva.sock");
        let server = Server::bind(&path).unwrap();
        let stopper = server.stopper();
        std::thread::spawn(move || {
            let _ = server.serve(handler);
        });
        // Дать серверу дойти до accept: connect до этого момента получит отказ.
        for _ in 0..100 {
            if Client::connect(&path).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        (path, dir, stopper)
    }

    #[test]
    fn a_command_makes_a_round_trip() {
        let handler = EchoHandler::new();
        let (path, _dir, stopper) = started(handler.clone());
        let mut client = Client::connect(&path).unwrap();
        let result = client
            .call_ok(Command::RecordStart {
                mode: Mode::Dictation,
                style: Some("cleanup".into()),
            })
            .unwrap();
        assert_eq!(result["started"], true);
        assert_eq!(handler.seen.lock().unwrap().len(), 1);
        stopper.stop();
    }

    #[test]
    fn several_commands_share_one_connection_and_keep_their_ids() {
        let (path, _dir, stopper) = started(EchoHandler::new());
        let mut client = Client::connect(&path).unwrap();
        let first = client.call(Command::Ping).unwrap();
        let second = client.call(Command::Status).unwrap();
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        assert!(first.ok && second.ok);
        stopper.stop();
    }

    #[test]
    fn a_daemon_error_reaches_the_client_with_its_code() {
        let (path, _dir, stopper) = started(EchoHandler::new());
        let mut client = Client::connect(&path).unwrap();
        let err = client
            .call_ok(Command::RecordStart {
                mode: Mode::Command,
                style: None,
            })
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::Busy));
        stopper.stop();
    }

    #[test]
    fn an_unknown_command_is_answered_with_bad_request_not_silence() {
        let (path, _dir, stopper) = started(EchoHandler::new());
        let name = name_for(&path).unwrap();
        let stream = Stream::connect(name).unwrap();
        let (recv, mut send) = stream.split();
        send.write_all(b"{\"v\":1,\"id\":9,\"cmd\":\"fly\"}\n")
            .unwrap();
        send.flush().unwrap();
        let mut line = String::new();
        BufReader::new(recv).read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, ErrorCode::BadRequest);
        stopper.stop();
    }

    #[test]
    fn a_subscriber_receives_events_as_they_are_published() {
        let handler = EchoHandler::new();
        let (tx, rx) = channel();
        *handler.events.lock().unwrap() = Some(rx);
        let (path, _dir, stopper) = started(handler.clone());
        let client = Client::connect(&path).unwrap();
        let mut events = client.subscribe(true).unwrap();
        tx.send(Event::Level { rms: 0.25 }).unwrap();
        tx.send(Event::ConfigReloaded).unwrap();
        assert_eq!(events.next(), Some(Event::Level { rms: 0.25 }));
        assert_eq!(events.next(), Some(Event::ConfigReloaded));
        drop(tx);
        assert_eq!(events.next(), None, "закрытый канал завершает подписку");
        stopper.stop();
    }

    #[test]
    fn ping_reports_the_pid_of_a_live_daemon() {
        let (path, _dir, stopper) = started(EchoHandler::new());
        assert_eq!(ping(&path), Some(4242));
        stopper.stop();
    }

    #[test]
    fn there_is_no_daemon_on_a_path_nobody_listens_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.sock");
        assert_eq!(ping(&path), None);
        let err = Client::connect(&path).unwrap_err();
        assert!(matches!(err, IpcClientError::NotRunning { .. }), "{err}");
    }

    #[test]
    fn a_dead_socket_file_does_not_block_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("molva.sock");
        std::fs::write(&path, b"").unwrap();
        // Файл есть, слушателя нет: сервер обязан подняться, а не сказать «адрес занят».
        let server = Server::bind(&path).expect("мёртвый сокет должен быть удалён");
        let stopper = server.stopper();
        std::thread::spawn(move || {
            let _ = server.serve(EchoHandler::new());
        });
        for _ in 0..100 {
            if Client::connect(&path).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ping(&path).is_some());
        stopper.stop();
    }

    #[test]
    fn the_default_socket_path_is_absolute_and_named_after_the_program() {
        let path = socket_path();
        assert!(
            path.is_absolute() || path.starts_with(r"\\.\pipe"),
            "{path:?}"
        );
        assert!(path.to_string_lossy().contains("molva"), "{path:?}");
    }
}
