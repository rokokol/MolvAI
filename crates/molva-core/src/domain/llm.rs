// SPDX-License-Identifier: MIT
//! Постобработка языковой моделью: контракт клиента.

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
    #[error("постобработка отключена")]
    Disabled,
    #[error("таймаут запроса к модели ({0} с)")]
    Timeout(u64),
    #[error("модель недоступна: {0}")]
    Unavailable(String),
    #[error("ошибка авторизации у провайдера")]
    Auth,
    #[error("некорректный ответ модели: {0}")]
    BadResponse(String),
}

/// Клиент языковой модели. Ошибка клиента никогда не теряет реплику: конвейер отдаёт сырой текст.
pub trait LlmClient: std::fmt::Debug + Send + Sync {
    /// Идентификатор провайдера для журнала, например `ollama`.
    fn id(&self) -> &str;
    /// Локальная ли модель: попадает в журнал полем `local_llm`.
    fn is_local(&self) -> bool;
    fn complete(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError>;
}
