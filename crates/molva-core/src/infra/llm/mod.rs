// SPDX-License-Identifier: MIT
//! Clients of language models. All providers use the OpenAI `/chat/completions` dialect,
//! so there is one client, and only the base URL, the model and the presence of a key differ

pub mod openai_compat;

pub use openai_compat::{OpenAiCompatClient, Provider};
