// SPDX-License-Identifier: MIT
//! Ядро MolvAI.
//!
//! Слои:
//! - `domain` — типы, трейты и чистые функции без зависимостей на фреймворки и железо;
//! - `app` — конвейер, демон, конфиг, журнал (появляются по мере реализации дорожек);
//! - `infra` — реальные реализации трейтов: cpal, whisper.cpp, HTTP, вставка, evdev;
//! - `ipc` — протокол между демоном, CLI и GUI.
//!
//! Железо тестируется через фейки трейтов из `domain::fakes`, а не через feature flags.

pub mod app;
pub mod config;
pub mod domain;
pub mod infra;
pub mod ipc;

pub use config::Config;
