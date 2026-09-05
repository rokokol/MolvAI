// SPDX-License-Identifier: MIT
//! Прикладной слой: сценарии поверх контрактов `domain` и реализаций `infra`.
//!
//! Объявления по одному на строку: файл общий для нескольких дорожек, так меньше конфликтов.

pub mod bench;
pub mod engine;
pub mod models;
pub mod wer;
