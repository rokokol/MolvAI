// SPDX-License-Identifier: MIT
//! Application layer: scenarios on top of the `domain` contracts and the `infra` implementations
//!
//! This layer holds the implementations that tests check with fakes and temporary directories:
//! the daemon and its state machine, the phrase pipeline, the journal, statistics, rules,
//! the dictionary, styles, silence trimming, models, bench. Declarations stay one per line,
//! because several work tracks share this file

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
