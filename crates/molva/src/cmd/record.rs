// SPDX-License-Identifier: MIT
//! `molva record start|stop|toggle|cancel` — тонкий клиент демона.
//!
//! Именно эти вызовы стоят в биндах композитора, поэтому команда обязана быть быстрой и
//! возвращать честный код: бинд не покажет пользователю текст ошибки, но код увидит скрипт.

use std::path::Path;

use molva_core::domain::entry::Mode;
use molva_core::infra::ipc::{Client, IpcClientError};
use molva_core::ipc::protocol::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Start,
    Stop,
    Toggle,
    Cancel,
}

/// Команда протокола для действия CLI.
pub(crate) fn command_for(action: Action, mode: Mode, style: Option<String>) -> Command {
    match action {
        Action::Start => Command::RecordStart { mode, style },
        Action::Stop => Command::RecordStop,
        Action::Toggle => Command::RecordToggle { mode, style },
        Action::Cancel => Command::RecordCancel,
    }
}

pub(crate) fn run(
    socket: &Path,
    action: Action,
    mode: Mode,
    style: Option<String>,
) -> Result<(), IpcClientError> {
    let mut client = Client::connect(socket)?;
    client.call_ok(command_for(action, mode, style))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_maps_to_its_protocol_command() {
        assert_eq!(
            command_for(Action::Start, Mode::Dictation, Some("formal".into())),
            Command::RecordStart {
                mode: Mode::Dictation,
                style: Some("formal".into())
            }
        );
        assert_eq!(
            command_for(Action::Stop, Mode::Dictation, None),
            Command::RecordStop
        );
        assert_eq!(
            command_for(Action::Toggle, Mode::Command, None),
            Command::RecordToggle {
                mode: Mode::Command,
                style: None
            }
        );
        assert_eq!(
            command_for(Action::Cancel, Mode::Dictation, None),
            Command::RecordCancel
        );
    }

    #[test]
    fn stop_and_cancel_ignore_mode_and_style() {
        // Режим и стиль запомнены при старте: передавать их снова — значит уметь их разойтись.
        assert_eq!(
            command_for(Action::Stop, Mode::Command, Some("x".into())),
            Command::RecordStop
        );
    }

    #[test]
    fn a_missing_daemon_is_reported_as_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let err = run(&socket, Action::Stop, Mode::Dictation, None).unwrap_err();
        assert!(matches!(err, IpcClientError::NotRunning { .. }), "{err}");
    }
}
