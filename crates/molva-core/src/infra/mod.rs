// SPDX-License-Identifier: MIT
//! Инфраструктура: реальные реализации контрактов `domain` поверх ОС, сети и внешних библиотек.
//!
//! Объявления по одному на строку: файл общий для нескольких дорожек, так меньше конфликтов.

pub mod audio;
pub mod hotkeys;
pub mod inject;
pub mod ipc;
pub mod llm;
pub mod notify;
pub mod platform;
pub mod sound;
pub mod stt;
