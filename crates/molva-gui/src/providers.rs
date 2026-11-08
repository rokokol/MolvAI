// SPDX-License-Identifier: MIT
//! Presets of the post-processing provider: what a choice fills in
//!
//! The base URL and the local flag come from `molva_core::infra::llm::Provider`, so the
//! settings page shows the same address the client connects to. Only the model name and the
//! environment variable of the key live here: the core has no default for them.

use molva_core::infra::llm::Provider;
use serde::Serialize;

/// One provider as the settings list shows it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmProviderOption {
    /// The value of `llm.provider`
    pub id: String,
    /// The value of `llm.base_url` that the provider expects
    pub base_url: String,
    /// A model that this provider serves, as a starting point
    pub model: String,
    /// The environment variable that `llm.api_key_source = "env"` reads
    pub api_key_env: String,
    /// A local provider needs no key and does not send the text away
    pub is_local: bool,
}

/// `llm.provider` → a starting model and a key variable
///
/// An unknown id falls into `custom`: the user sets every field by hand. `Provider::parse`
/// makes the same decision, so the URL and the local flag always agree with the client.
const PRESETS: [(&str, &str, &str); 6] = [
    ("ollama", "qwen3.5:4b", ""),
    ("lmstudio", "local-model", ""),
    ("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
    ("openrouter", "z-ai/glm-5.3-flash", "OPENROUTER_API_KEY"),
    ("groq", "llama-3.3-70b-versatile", "GROQ_API_KEY"),
    ("custom", "", ""),
];

/// The providers the settings page offers, in the order of the list above
pub fn llm_providers() -> Vec<LlmProviderOption> {
    PRESETS
        .iter()
        .map(|(id, model, api_key_env)| {
            let provider = Provider::parse(id);
            LlmProviderOption {
                id: (*id).to_string(),
                base_url: provider.default_base_url().to_string(),
                model: (*model).to_string(),
                api_key_env: (*api_key_env).to_string(),
                is_local: provider.is_local(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_carries_the_url_the_client_uses() {
        for option in llm_providers() {
            assert_eq!(
                option.base_url,
                Provider::parse(&option.id).default_base_url(),
                "{}",
                option.id
            );
            assert_eq!(
                option.is_local,
                Provider::parse(&option.id).is_local(),
                "{}",
                option.id
            );
        }
    }

    #[test]
    fn the_settings_page_offers_the_local_and_the_cloud_providers() {
        let ids: Vec<String> = llm_providers().into_iter().map(|o| o.id).collect();
        for wanted in ["ollama", "lmstudio", "openai", "openrouter"] {
            assert!(ids.iter().any(|id| id == wanted), "{wanted} is missing");
        }
        let openai = llm_providers()
            .into_iter()
            .find(|o| o.id == "openai")
            .expect("OpenAI is in the list");
        assert!(!openai.is_local);
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
        assert_eq!(openai.api_key_env, "OPENAI_API_KEY");
    }

    #[test]
    fn a_local_provider_has_no_key_variable_and_a_cloud_one_has() {
        for option in llm_providers() {
            if option.is_local {
                assert!(
                    option.api_key_env.is_empty(),
                    "{} must need no key",
                    option.id
                );
            }
        }
    }
}
