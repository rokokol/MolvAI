// SPDX-License-Identifier: MIT
//! `molva record start|stop|toggle|cancel` — a thin client of the daemon
//!
//! These exact calls stand in the binds of the compositor, so the command must be fast and
//! return an honest code: a bind does not show the error text to the user, but a script sees the code

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

/// The protocol command for a CLI action
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
        // The mode and the style are remembered at the start: to pass them again means to let them diverge
        assert_eq!(
            command_for(Action::Stop, Mode::Command, Some("x".into())),
            Command::RecordStop
        );
    }

    #[test]
    fn a_missing_daemon_is_reported_as_not_running() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent.sock");
        let error = run(&socket, Action::Stop, Mode::Dictation, None).unwrap_err();
        assert!(
            matches!(error, IpcClientError::NotRunning { .. }),
            "{error}"
        );
    }
}
