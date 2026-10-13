// SPDX-License-Identifier: MIT
//! Domain: the contracts frozen at hour 0 of the hackathon
//!
//! Changes here are additive only, and only through the coordinator

pub mod audio;
pub mod clock;
pub mod entry;
pub mod fakes;
pub mod hotkeys;
pub mod inject;
pub mod journal;
pub mod llm;
pub mod notify;
pub mod sound;
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
pub use sound::{CueKind, SoundCue};
pub use stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};
pub use text::{word_count, Style, TextRule};
