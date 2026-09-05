// SPDX-License-Identifier: MIT
//! `molva status` — состояние демона одной строкой, JSON-ом или потоком событий.

use std::path::Path;

use molva_core::infra::ipc::{Client, IpcClientError};
use molva_core::ipc::protocol::{Command, Event};
use serde_json::Value;

/// Человекочитаемая строка состояния.
pub fn describe(status: &Value) -> String {
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("неизвестно");
    let pid = status.get("pid").and_then(Value::as_u64).unwrap_or(0);
    format!("состояние: {} (pid {pid})", human_state(state))
}

/// Состояния протокола по-русски: их видит пользователь, а не разработчик.
pub fn human_state(state: &str) -> &str {
    match state {
        "idle" => "готов",
        "recording" => "идёт запись",
        "transcribing" => "распознаю",
        "post_processing" => "обрабатываю",
        "injecting" => "вставляю",
        other => other,
    }
}

/// Короткая строка про событие для `--watch`.
pub fn describe_event(event: &Event) -> Option<String> {
    match event {
        Event::State { state, .. } => {
            let name = serde_json::to_value(state)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            Some(human_state(&name).to_string())
        }
        Event::Entry { entry } => Some(format!(
            "реплика: {} ({} слов, {} мс)",
            entry.text_final.as_deref().unwrap_or("<без текста>"),
            entry.words,
            entry.latency_ms.total
        )),
        Event::Error { message, .. } => Some(format!("ошибка: {message}")),
        // Уровень сигнала идёт десятками раз в секунду: в текстовый вывод он не помещается.
        Event::Level { .. } | Event::Hypothesis { .. } => None,
        Event::ConfigReloaded => Some("настройки перечитаны".into()),
        Event::DevicesChanged => Some("список устройств изменился".into()),
    }
}

pub fn run(socket: &Path, json: bool, watch: bool) -> Result<(), IpcClientError> {
    let mut client = Client::connect(socket)?;
    let status = client.call_ok(Command::Status)?;
    if json {
        println!("{status}");
    } else {
        println!("{}", describe(&status));
    }
    if !watch {
        return Ok(());
    }
    let events = Client::connect(socket)?.subscribe(false)?;
    for event in events {
        if json {
            if let Ok(line) = serde_json::to_string(&event) {
                println!("{line}");
            }
        } else if let Some(line) = describe_event(&event) {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::ipc::protocol::DaemonState;

    #[test]
    fn status_is_rendered_with_state_and_pid() {
        let status = serde_json::json!({ "state": "recording", "pid": 17 });
        assert_eq!(describe(&status), "состояние: идёт запись (pid 17)");
    }

    #[test]
    fn an_unknown_state_is_shown_as_is_instead_of_being_hidden() {
        let status = serde_json::json!({ "state": "warming_up", "pid": 1 });
        assert!(describe(&status).contains("warming_up"));
    }

    #[test]
    fn level_events_are_not_printed_but_replies_are() {
        assert_eq!(describe_event(&Event::Level { rms: 0.4 }), None);
        assert_eq!(
            describe_event(&Event::State {
                state: DaemonState::Injecting,
                mode: None
            }),
            Some("вставляю".to_string())
        );
        assert_eq!(
            describe_event(&Event::Error {
                code: molva_core::ipc::protocol::ErrorCode::InjectFailed,
                message: "нет окна".into(),
                hint: None
            }),
            Some("ошибка: нет окна".to_string())
        );
    }
}
