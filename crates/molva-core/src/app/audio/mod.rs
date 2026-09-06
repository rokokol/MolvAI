// SPDX-License-Identifier: MIT
//! Обработка записанного сигнала до распознавания.

pub mod segmenter;
pub mod trim;

pub use segmenter::{Chunk, Segmenter, SegmenterConfig};
pub use trim::{is_silent, peak_db, trim_for_config, trim_silence};
