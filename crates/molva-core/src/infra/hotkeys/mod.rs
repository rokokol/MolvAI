// SPDX-License-Identifier: MIT
//! Hotkey sources
//!
//! The main path on Wayland is compositor binds that call `molva record ...` (see
//! `docs/hotkeys-wayland.md`): the compositor already holds the keyboard focus, so it needs no
//! permission to read input devices. `evdev` is the fallback for environments with no binds

#[cfg(target_os = "linux")]
pub mod evdev_source;

pub use crate::app::hotkeys::{specs_from_config, HotkeySpec, Modifier};
