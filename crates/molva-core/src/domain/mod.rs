// SPDX-License-Identifier: MIT
//! Предметная область: контракты, замороженные на час 0 хакатона.
//!
//! Изменения здесь только аддитивные и только через координатора.

pub mod audio;
pub mod clock;
pub mod entry;
pub mod fakes;
pub mod hotkeys;
pub mod inject;
pub mod journal;
pub mod llm;
pub mod notify;
pub mod stt;
pub mod text;

pub use audio::{AudioError, AudioSource, DeviceInfo, PcmAudio, TARGET_SAMPLE_RATE};
pub use clock::{Clock, SystemClock};
pub use entry::{Entry, LatencyMs, Mode, Source, Tokens};
pub use hotkeys::{HotkeyAction, HotkeyError, HotkeyEvent, HotkeySource, KeyState};
pub use inject::{InjectError, InjectReport, OutputMode, TextInjector};
pub use journal::{Journal, JournalError};
pub use llm::{ChatRequest, ChatResponse, LlmClient, LlmError};
pub use notify::Notifier;
pub use stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};
pub use text::{word_count, Style, TextRule};
