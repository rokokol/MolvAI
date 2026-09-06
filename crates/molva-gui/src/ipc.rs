// SPDX-License-Identifier: MIT
//! Минимальный клиент IPC демона: newline-JSON поверх локального сокета.
//!
//! Заменяется на `molva_core::infra::ipc::Client`, когда дорожка F вынесет транспорт в ядро;
//! здесь сознательно только то, что нужно GUI: разовый запрос и поток подписки.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use interprocess::local_socket::{prelude::*, Stream};
use molva_core::ipc::{Command, ErrorCode, Event, IpcError, Request, Response};
use serde_json::Value;
use thiserror::Error;

/// Счётчик идентификаторов запросов на весь процесс: ответы сопоставляются по нему.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum IpcClientError {
    #[error("демон не запущен ({path}): {reason}")]
    NotRunning { path: String, reason: String },
    #[error("демон закрыл соединение")]
    Disconnected,
    #[error("ошибка обмена с демоном: {0}")]
    Io(String),
    #[error("демон прислал не то, что описано протоколом: {0}")]
    Protocol(String),
    #[error("{0}")]
    Daemon(#[from] DaemonError),
    #[error("не удалось определить путь к сокету демона: {0}")]
    NoSocketPath(String),
}

impl IpcClientError {
    /// Что предложить пользователю следующим шагом — попадает в уведомление и в UI.
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::NotRunning { .. } | Self::Disconnected => {
                Some("Запустите демон кнопкой «Запустить демон» или командой `molva daemon`".into())
            }
            Self::Daemon(err) => err.hint.clone(),
            _ => None,
        }
    }

    /// Отличает «демона нет» от «демон ответил ошибкой»: UI показывает их по-разному.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::NotRunning { .. } | Self::Disconnected)
    }
}

/// Ошибка, пришедшая от демона по протоколу.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct DaemonError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
}

impl From<IpcError> for DaemonError {
    fn from(err: IpcError) -> Self {
        Self {
            code: err.code,
            message: err.message,
            hint: err.hint,
        }
    }
}

/// Путь к сокету демона: `$XDG_RUNTIME_DIR/molva.sock`, `$TMPDIR/molva.sock`, `\\.\pipe\molva`.
pub fn socket_path() -> Result<PathBuf, IpcClientError> {
    if let Some(path) = std::env::var_os("MOLVA_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    {
        Ok(PathBuf::from(r"\\.\pipe\molva"))
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into());
        Ok(PathBuf::from(dir).join("molva.sock"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            IpcClientError::NoSocketPath("переменная XDG_RUNTIME_DIR не задана".into())
        })?;
        Ok(PathBuf::from(dir).join("molva.sock"))
    }
}

/// Сообщение от демона: ответ на запрос или событие подписки.
#[derive(Debug)]
pub enum Message {
    Response(Response),
    Event(Event),
}

/// Одно соединение с демоном. Разовый запрос закрывает его сразу, подписка — держит.
#[derive(Debug)]
pub struct Connection {
    reader: BufReader<Stream>,
}

impl Connection {
    /// Соединение с сокетом демона по пути из окружения.
    pub fn connect() -> Result<Self, IpcClientError> {
        Self::connect_at(&socket_path()?)
    }

    /// Соединение по конкретному пути: так тесты подставляют свой сокет,
    /// не трогая переменные окружения процесса.
    pub fn connect_at(path: &Path) -> Result<Self, IpcClientError> {
        let display = path.display().to_string();
        // Имя строит ядро: на Windows путь из tempdir или `MOLVA_SOCKET` должен превратиться
        // в то же имя канала, что и у демона.
        let name = molva_core::infra::ipc::transport::socket_name(path)
            .map_err(|e| IpcClientError::NoSocketPath(e.to_string()))?;
        let stream = Stream::connect(name).map_err(|e| IpcClientError::NotRunning {
            path: display,
            reason: e.to_string(),
        })?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    /// Отправить команду, не дожидаясь ответа: так открывается подписка.
    pub fn send(&mut self, cmd: Command) -> Result<u64, IpcClientError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut line = serde_json::to_string(&Request::new(id, cmd))
            .map_err(|e| IpcClientError::Protocol(e.to_string()))?;
        line.push('\n');
        let stream = self.reader.get_mut();
        stream
            .write_all(line.as_bytes())
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        stream
            .flush()
            .map_err(|e| IpcClientError::Io(e.to_string()))?;
        Ok(id)
    }

    /// Прочитать следующее сообщение; `None` — соединение закрыто демоном.
    pub fn recv(&mut self) -> Result<Option<Message>, IpcClientError> {
        loop {
            let mut line = String::new();
            let read = self
                .reader
                .read_line(&mut line)
                .map_err(|e| IpcClientError::Io(e.to_string()))?;
            if read == 0 {
                return Ok(None);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return Ok(Some(parse_message(trimmed)?));
        }
    }

    /// Команда и ответ на неё; события, пришедшие между ними, пропускаются.
    pub fn request(&mut self, cmd: Command) -> Result<Value, IpcClientError> {
        let id = self.send(cmd)?;
        loop {
            match self.recv()? {
                None => return Err(IpcClientError::Disconnected),
                Some(Message::Event(_)) => {}
                Some(Message::Response(resp)) if resp.id != id => {}
                Some(Message::Response(resp)) => return unwrap_response(resp),
            }
        }
    }
}

/// Разбор строки протокола: сначала событие (у него есть поле `event`), потом ответ.
pub fn parse_message(line: &str) -> Result<Message, IpcClientError> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| IpcClientError::Protocol(e.to_string()))?;
    if value.get("event").is_some() {
        let event: Event = serde_json::from_value(value)
            .map_err(|e| IpcClientError::Protocol(format!("событие: {e}")))?;
        return Ok(Message::Event(event));
    }
    let response: Response = serde_json::from_value(value)
        .map_err(|e| IpcClientError::Protocol(format!("ответ: {e}")))?;
    Ok(Message::Response(response))
}

/// Успешный ответ отдаёт `result` (или `null`), неуспешный — ошибку демона.
pub fn unwrap_response(resp: Response) -> Result<Value, IpcClientError> {
    if resp.ok {
        return Ok(resp.result.unwrap_or(Value::Null));
    }
    let error = resp.error.unwrap_or(IpcError {
        code: ErrorCode::Internal,
        message: "демон вернул ошибку без описания".into(),
        hint: None,
    });
    Err(IpcClientError::Daemon(error.into()))
}

/// Разовый запрос: соединение открывается и закрывается вокруг него.
pub fn request(cmd: Command) -> Result<Value, IpcClientError> {
    Connection::connect()?.request(cmd)
}

/// Отвечает ли демон на `ping`.
pub fn is_running() -> bool {
    request(Command::Ping).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::ipc::DaemonState;

    #[test]
    fn event_line_is_recognised_before_response() {
        let msg = parse_message(r#"{"event":"level","rms":0.25}"#).unwrap();
        match msg {
            Message::Event(Event::Level { rms }) => assert_eq!(rms, 0.25),
            other => panic!("ожидалось событие уровня, получено {other:?}"),
        }
    }

    #[test]
    fn state_event_carries_daemon_state() {
        let msg = parse_message(r#"{"event":"state","state":"recording"}"#).unwrap();
        match msg {
            Message::Event(Event::State { state, .. }) => {
                assert_eq!(state, DaemonState::Recording);
            }
            other => panic!("ожидалось событие состояния, получено {other:?}"),
        }
    }

    #[test]
    fn response_line_is_parsed_as_response() {
        let msg = parse_message(r#"{"id":3,"ok":true,"result":{"state":"idle"}}"#).unwrap();
        match msg {
            Message::Response(resp) => {
                assert_eq!(resp.id, 3);
                assert!(resp.ok);
            }
            other @ Message::Event(_) => panic!("ожидался ответ, получено {other:?}"),
        }
    }

    #[test]
    fn garbage_line_is_a_protocol_error_not_a_panic() {
        let err = parse_message("не json").unwrap_err();
        assert!(matches!(err, IpcClientError::Protocol(_)), "{err}");
    }

    #[test]
    fn error_response_becomes_daemon_error_with_hint() {
        let resp: Response = serde_json::from_str(
            r#"{"id":1,"ok":false,"error":{"code":"no_device","message":"нет микрофона","hint":"выберите устройство"}}"#,
        )
        .unwrap();
        let err = unwrap_response(resp).unwrap_err();
        assert_eq!(err.hint().as_deref(), Some("выберите устройство"));
        assert!(!err.is_unavailable());
    }

    #[test]
    fn error_response_without_details_still_reports_something() {
        let resp = Response {
            id: 1,
            ok: false,
            result: None,
            error: None,
        };
        let err = unwrap_response(resp).unwrap_err();
        assert!(err.to_string().contains("ошибку"), "{err}");
    }

    #[test]
    fn missing_daemon_offers_starting_it() {
        let err = IpcClientError::NotRunning {
            path: "/run/user/1000/molva.sock".into(),
            reason: "нет такого файла".into(),
        };
        assert!(err.is_unavailable());
        assert!(err.hint().unwrap().contains("молва") || err.hint().unwrap().contains("molva"));
    }
}
