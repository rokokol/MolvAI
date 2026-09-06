// SPDX-License-Identifier: MIT
//! Прикладной слой: сценарии поверх контрактов `domain` и реализаций `infra`.
//!
//! Здесь живут реализации, которые проверяются фейками и временными каталогами: демон и его
//! машина состояний, конвейер реплики, журнал, статистика, правила, словарь, стили, обрезка
//! тишины, модели, bench. Объявления по одному на строку: файл общий для нескольких дорожек.

pub mod audio;
pub mod bench;
pub mod daemon;
pub mod dictionary;
pub mod engine;
pub mod hotkeys;
pub mod journal;
pub mod journal_crypto;
pub mod llm_output;
pub mod models;
pub mod pipeline;
pub mod rules;
pub mod secrets;
pub mod stats;
pub mod styles;
pub mod wer;
