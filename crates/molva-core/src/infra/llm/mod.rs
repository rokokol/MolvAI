// SPDX-License-Identifier: MIT
//! Клиенты языковых моделей. Все провайдеры говорят на диалекте OpenAI `/chat/completions`,
//! поэтому клиент один, а различается только базовый адрес, модель и наличие ключа.

pub mod openai_compat;

pub use openai_compat::{OpenAiCompatClient, Provider};
