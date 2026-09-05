// SPDX-License-Identifier: MIT
//! Обработка записанного сигнала до распознавания.

pub mod trim;

pub use trim::{is_silent, peak_db, trim_for_config, trim_silence};
