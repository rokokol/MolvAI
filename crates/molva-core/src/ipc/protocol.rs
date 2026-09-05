// SPDX-License-Identifier: MIT
//! Протокол IPC: JSON, одна строка — одно сообщение.
//!
//! Запрос: `{"v":1,"id":7,"cmd":"record.start","args":{"mode":"dictation"}}`.
//! Ответ: `{"id":7,"ok":true,"result":{...}}` или `{"id":7,"ok":false,"error":{...}}`.
//! События на подписанных соединениях: `{"event":"level","rms":0.08}`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::entry::{Entry, Mode};

pub const PROTOCOL_VERSION: u32 = 1;

/// Состояние демона, видимое клиентам.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Idle,
    Recording,
    Transcribing,
    PostProcessing,
    Injecting,
}

/// Команды клиента. Имена с точкой — это пространства имён, как в CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args")]
pub enum Command {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "record.start")]
    RecordStart {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    #[serde(rename = "record.stop")]
    RecordStop,
    #[serde(rename = "record.toggle")]
    RecordToggle {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    #[serde(rename = "record.cancel")]
    RecordCancel,
    #[serde(rename = "style.set")]
    StyleSet { style: String },
    #[serde(rename = "style.next")]
    StyleNext,
    #[serde(rename = "config.reload")]
    ConfigReload,
    #[serde(rename = "devices.list")]
    DevicesList,
    #[serde(rename = "dictionary.reload")]
    DictionaryReload,
    #[serde(rename = "inject.text")]
    InjectText {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<crate::domain::inject::OutputMode>,
    },
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(default)]
        levels: bool,
    },
    #[serde(rename = "shutdown")]
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

impl Request {
    pub fn new(id: u64, cmd: Command) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            cmd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Busy,
    NotRecording,
    NoDevice,
    SttFailed,
    LlmFailed,
    InjectFailed,
    BadRequest,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
    /// Что сделать дальше — попадает в уведомление пользователю.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, code: ErrorCode, message: impl Into<String>, hint: Option<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(IpcError {
                code,
                message: message.into(),
                hint,
            }),
        }
    }
}

/// События для подписчиков.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    State {
        state: DaemonState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<Mode>,
    },
    Level {
        rms: f32,
    },
    /// Черновик текста во время записи (потоковый предпросмотр).
    Hypothesis {
        text: String,
    },
    /// Запись в коробке: она на порядок больше остальных событий.
    Entry {
        entry: Box<Entry>,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    ConfigReloaded,
    DevicesChanged,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(cmd: &Command) {
        let req = Request::new(1, cmd.clone());
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(&back.cmd, cmd, "не совпало после round-trip: {json}");
    }

    #[test]
    fn every_command_round_trips() {
        round_trip(&Command::Ping);
        round_trip(&Command::Status);
        round_trip(&Command::RecordStart {
            mode: Mode::Dictation,
            style: Some("cleanup".into()),
        });
        round_trip(&Command::RecordStop);
        round_trip(&Command::RecordToggle {
            mode: Mode::Command,
            style: None,
        });
        round_trip(&Command::RecordCancel);
        round_trip(&Command::StyleSet {
            style: "formal".into(),
        });
        round_trip(&Command::StyleNext);
        round_trip(&Command::ConfigReload);
        round_trip(&Command::DevicesList);
        round_trip(&Command::DictionaryReload);
        round_trip(&Command::InjectText {
            text: "текст".into(),
            mode: Some(crate::domain::inject::OutputMode::Paste),
        });
        round_trip(&Command::Subscribe { levels: true });
        round_trip(&Command::Shutdown);
    }

    #[test]
    fn request_wire_format_matches_documentation() {
        let req = Request::new(
            7,
            Command::RecordStart {
                mode: Mode::Dictation,
                style: None,
            },
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["id"], 7);
        assert_eq!(json["cmd"], "record.start");
        assert_eq!(json["args"]["mode"], "dictation");
        let ping = serde_json::to_string(&Request::new(1, Command::Ping)).unwrap();
        assert_eq!(ping, r#"{"v":1,"id":1,"cmd":"ping"}"#);
    }

    #[test]
    fn unknown_command_is_rejected() {
        let err = serde_json::from_str::<Request>(r#"{"v":1,"id":1,"cmd":"fly"}"#);
        assert!(err.is_err());
    }

    #[test]
    fn error_response_carries_code_and_hint() {
        let resp = Response::err(
            3,
            ErrorCode::Busy,
            "запись уже идёт",
            Some("нажмите клавишу ещё раз, чтобы остановить".into()),
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "busy");
        assert!(json.get("result").is_none());
        let back: Response = serde_json::from_value(json).unwrap();
        assert_eq!(back, resp);
    }

    #[test]
    fn events_are_tagged_by_name() {
        let level = serde_json::to_value(Event::Level { rms: 0.5 }).unwrap();
        assert_eq!(level["event"], "level");
        let state = serde_json::to_value(Event::State {
            state: DaemonState::PostProcessing,
            mode: Some(Mode::Dictation),
        })
        .unwrap();
        assert_eq!(state["state"], "post_processing");
        let reloaded = serde_json::to_string(&Event::ConfigReloaded).unwrap();
        assert_eq!(reloaded, r#"{"event":"config_reloaded"}"#);
    }
}
