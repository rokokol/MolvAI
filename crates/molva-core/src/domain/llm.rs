// SPDX-License-Identifier: MIT
//! Post-processing by a language model: the client contract

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub system: String,
    pub user: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChatResponse {
    pub text: String,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LlmError {
    #[error("post-processing is disabled")]
    Disabled,
    #[error("model request timed out ({0} s)")]
    Timeout(u64),
    #[error("model is unavailable: {0}")]
    Unavailable(String),
    #[error("provider authorization error")]
    Auth,
    #[error("invalid model response: {0}")]
    BadResponse(String),
}

/// Client of a language model. A client error never loses a phrase: the pipeline returns the raw
/// text
pub trait LlmClient: std::fmt::Debug + Send + Sync {
    /// Provider identifier for the journal, for example `ollama`
    fn id(&self) -> &str;
    /// Whether the model is local: it goes to the journal in the `local_llm` field
    fn is_local(&self) -> bool;
    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError>;
}
