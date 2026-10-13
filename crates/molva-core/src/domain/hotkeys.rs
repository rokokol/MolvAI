// SPDX-License-Identifier: MIT
//! Hotkeys: the events and the source contract

use std::sync::mpsc::Sender;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    /// Key hold: pressing starts the recording, releasing starts the processing
    PushToTalk,
    /// One press starts, the second press stops
    Toggle,
    /// Mode that edits the selected text by voice
    Command,
    /// Cancel the current phrase
    Cancel,
    /// Switch the style profile
    StyleNext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Pressed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyEvent {
    pub action: HotkeyAction,
    pub state: KeyState,
    pub at: Instant,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HotkeyError {
    #[error("key combination not recognized: {0}")]
    BadSpec(String),
    #[error("key combination is taken by the system: {0}")]
    Conflict(String),
    #[error("no permission to read input devices: {0}")]
    Permission(String),
    #[error("hotkeys are not available in this environment: {0}")]
    Unsupported(String),
    #[error("hotkey backend error: {0}")]
    Backend(String),
}

/// Source of key events: evdev on Linux, a Tauri plugin on Windows/macOS
///
/// Compositor binds on Wayland do not implement this trait: they call the CLI, and the CLI sends IPC
pub trait HotkeySource: std::fmt::Debug + Send {
    /// Blocking loop. It sends events to `tx` until the channel closes or an error occurs
    fn run(self: Box<Self>, tx: Sender<HotkeyEvent>) -> Result<(), HotkeyError>;
}
