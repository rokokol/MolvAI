// SPDX-License-Identifier: MIT
//! One transcribed file: the result that the CLI prints as JSON and the GUI reads back
//!
//! The type lives in the core because both frontends cross a process boundary with it. A
//! second copy on either side would be a second spelling of the same JSON.

use serde::{Deserialize, Serialize};

use crate::domain::stt::Segment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileResult {
    /// Source path, or the stdin label
    pub file: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,
    pub audio_secs: f32,
    pub latency_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}
