// SPDX-License-Identifier: MIT
//! Journal of phrases: the write contract. The file implementation (JSONL) lives in `app`

use thiserror::Error;

use super::entry::Entry;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum JournalError {
    #[error("journal I/O error: {0}")]
    Io(String),
    #[error("could not serialize the entry: {0}")]
    Serialize(String),
    #[error("journal line {line} is corrupt")]
    Corrupt { line: usize },
}

/// Sink of entries. One entry is one phrase; the journal is append-only
pub trait Journal: std::fmt::Debug + Send {
    fn append(&mut self, entry: &Entry) -> Result<(), JournalError>;
}
