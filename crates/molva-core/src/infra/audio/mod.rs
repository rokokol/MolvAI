// SPDX-License-Identifier: MIT
//! Аудио-инфраструктура: захват с микрофона через cpal и чтение файлов.
//!
//! Объявления по одному на строку: файл общий для нескольких дорожек, так меньше конфликтов.

pub mod cpal_source;
pub mod decode;
pub mod level;

pub use cpal_source::{list_input_devices, CpalSource};
pub use level::ZeroLevelWatch;
