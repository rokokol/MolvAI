// SPDX-License-Identifier: MIT
//! Горячие клавиши: события и контракт источника.

use std::sync::mpsc::Sender;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    /// Удержание: нажатие начинает запись, отпускание запускает обработку.
    PushToTalk,
    /// Одно нажатие — старт, второе — стоп.
    Toggle,
    /// Режим правки выделенного текста голосом.
    Command,
    /// Отмена текущей реплики.
    Cancel,
    /// Переключение профиля стиля.
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
    #[error("комбинация не распознана: {0}")]
    BadSpec(String),
    #[error("комбинация занята системой: {0}")]
    Conflict(String),
    #[error("нет прав на чтение устройств ввода: {0}")]
    Permission(String),
    #[error("горячие клавиши недоступны в этом окружении: {0}")]
    Unsupported(String),
    #[error("ошибка бэкенда горячих клавиш: {0}")]
    Backend(String),
}

/// Источник событий клавиш: evdev на Linux, плагин Tauri на Windows/macOS.
///
/// Бинды композитора на Wayland не реализуют этот трейт: они вызывают CLI, а тот шлёт IPC.
pub trait HotkeySource: Send {
    /// Блокирующий цикл, шлёт события в `tx` до закрытия канала или ошибки.
    fn run(self: Box<Self>, tx: Sender<HotkeyEvent>) -> Result<(), HotkeyError>;
}
