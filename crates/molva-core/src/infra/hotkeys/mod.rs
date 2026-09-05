// SPDX-License-Identifier: MIT
//! Источники горячих клавиш.
//!
//! Основной путь на Wayland — бинды композитора, вызывающие `molva record ...` (см.
//! `docs/hotkeys-wayland.md`): композитор уже держит клавиатурный фокус, и ему не нужны права на
//! устройства ввода. `evdev` — запасной путь для окружений, где биндов нет.

#[cfg(target_os = "linux")]
pub mod evdev_source;

pub use crate::app::hotkeys::{specs_from_config, HotkeySpec, Modifier};
