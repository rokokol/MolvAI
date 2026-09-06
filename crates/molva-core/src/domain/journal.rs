// SPDX-License-Identifier: MIT
//! Журнал реплик: контракт записи. Файловая реализация (JSONL) живёт в `app`.

use thiserror::Error;

use super::entry::Entry;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum JournalError {
    #[error("ошибка ввода-вывода журнала: {0}")]
    Io(String),
    #[error("не удалось сериализовать запись: {0}")]
    Serialize(String),
    #[error("повреждена строка {line} журнала")]
    Corrupt { line: usize },
}

/// Приёмник записей. Одна запись — одна реплика; журнал только дописывается.
pub trait Journal: std::fmt::Debug + Send {
    fn append(&mut self, entry: &Entry) -> Result<(), JournalError>;
}
