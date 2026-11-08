// SPDX-License-Identifier: MIT
//! Client of `/chat/completions` in the OpenAI dialect: Ollama, LM Studio, OpenRouter, Groq, OpenAI
//!
//! The request is synchronous (`reqwest::blocking`): the core has no async runtime, and the
//! phrase waits for the model answer anyway. Retries live in the pipeline, and here is one
//! attempt — so the timeout means exactly what is configured, not "the timeout times the number
//! of attempts"
//!
//! The key is stored in [`ApiKey`], so the `Debug` of the client prints a mask, not the secret

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::app::secrets::ApiKey;
use crate::config::{provider_slugs, LlmConfig};
use crate::domain::llm::{ChatRequest, ChatResponse, LlmClient, LlmError};

/// Known providers: presets of the base URL and of the local flag
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    LmStudio,
    OpenRouter,
    Groq,
    OpenAi,
    Custom,
}

impl Provider {
    /// Parse the value of `llm.provider`; an unknown name is `Custom`, not an error
    pub fn parse(value: &str) -> Self {
        match value
            .trim()
            .to_lowercase()
            .replace(['-', '_', ' '], "")
            .as_str()
        {
            "ollama" => Provider::Ollama,
            "lmstudio" => Provider::LmStudio,
            "openrouter" => Provider::OpenRouter,
            "groq" => Provider::Groq,
            "openai" => Provider::OpenAi,
            _ => Provider::Custom,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Provider::Ollama => "ollama",
            Provider::LmStudio => "lmstudio",
            Provider::OpenRouter => "openrouter",
            Provider::Groq => "groq",
            Provider::OpenAi => "openai",
            Provider::Custom => "custom",
        }
    }

    /// Default URL; for `Custom` it is empty — the user must set it
    pub fn default_base_url(&self) -> &'static str {
        match self {
            Provider::Ollama => "http://localhost:11434/v1",
            Provider::LmStudio => "http://localhost:1234/v1",
            Provider::OpenRouter => "https://openrouter.ai/api/v1",
            Provider::Groq => "https://api.groq.com/openai/v1",
            Provider::OpenAi => "https://api.openai.com/v1",
            Provider::Custom => "",
        }
    }

    /// Whether the model is local: it goes to the journal in the `local_llm` field (criterion S-08)
    pub fn is_local(&self) -> bool {
        matches!(self, Provider::Ollama | Provider::LmStudio)
    }
}

/// Client of one provider
pub struct OpenAiCompatClient {
    base_url: String,
    model: String,
    api_key: Option<ApiKey>,
    provider_id: String,
    is_local: bool,
    timeout: Duration,
    /// The `reasoning_effort` field of the request; `None` means do not send (see
    /// `LlmConfig::reasoning_effort`)
    reasoning_effort: Option<String>,
    /// OpenRouter's `provider.data_collection`; `None` means do not send
    data_collection: Option<String>,
    /// OpenRouter's `provider.zdr`: ask for zero data retention
    zdr: bool,
    /// OpenRouter's `provider.order`: provider slugs in the order to try them; empty means the
    /// router picks
    provider_order: Vec<String>,
    /// OpenRouter's `provider.allow_fallbacks`
    allow_fallbacks: bool,
    /// OpenRouter's `provider.require_parameters`
    require_parameters: bool,
    http: reqwest::blocking::Client,
}

/// Whether a URL names this machine: the model then runs beside the daemon, not in a cloud
///
/// The host is taken from the text and not parsed by a URL library: a settings value without
/// a scheme, `localhost:11434`, must reach the same answer as the full form
pub fn is_loopback_url(base_url: &str) -> bool {
    let rest = base_url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // `[::1]:1234` keeps its brackets, `localhost:11434` loses the port
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"
    ) || host.to_ascii_lowercase().starts_with("127.")
}

/// What to send in `reasoning_effort` by the setting and the provider type. `auto` turns off
/// reasoning in local models (`none`) and does not send the field to cloud ones — it is
/// not valid for all of their models. An empty string means do not send, and anything else goes
/// as it is. `none` is the default of the settings, and OpenRouter drops it from the body on its
/// own, because its models may refuse to stop reasoning
pub fn effective_reasoning_effort(setting: &str, is_local: bool) -> Option<String> {
    match setting.trim() {
        "" => None,
        "auto" => is_local.then(|| "none".to_string()),
        other => Some(other.to_string()),
    }
}

impl fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCompatClient")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("is_local", &self.is_local)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatClient {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
        timeout: Duration,
        provider_id: impl Into<String>,
        is_local: bool,
    ) -> Result<Self, LlmError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| LlmError::Unavailable(with_causes(&e)))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key: api_key.map(ApiKey::new).filter(|key| !key.is_empty()),
            provider_id: provider_id.into(),
            is_local,
            timeout,
            reasoning_effort: None,
            data_collection: None,
            zdr: false,
            provider_order: Vec::new(),
            allow_fallbacks: true,
            require_parameters: false,
            http,
        })
    }

    /// Set `reasoning_effort` of the request; `None` means the field is not sent
    #[must_use]
    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    /// Set OpenRouter's data-collection policy; other providers never send it
    #[must_use]
    pub fn with_data_collection(mut self, policy: Option<String>) -> Self {
        self.data_collection = policy;
        self
    }

    /// Ask OpenRouter to keep no copy of the request
    #[must_use]
    pub fn with_zdr(mut self, zdr: bool) -> Self {
        self.zdr = zdr;
        self
    }

    /// Set OpenRouter's provider order; an empty list means the router picks
    #[must_use]
    pub fn with_provider_order(mut self, order: Vec<String>) -> Self {
        self.provider_order = order;
        self
    }

    /// Let OpenRouter fall back to another provider
    #[must_use]
    pub fn with_allow_fallbacks(mut self, allow: bool) -> Self {
        self.allow_fallbacks = allow;
        self
    }

    /// Ask OpenRouter for providers that support every request parameter
    #[must_use]
    pub fn with_require_parameters(mut self, require: bool) -> Self {
        self.require_parameters = require;
        self
    }

    /// Client from the settings: an empty `base_url` comes from the provider preset
    ///
    /// A custom provider that points at this machine is a local model too: it is one process
    /// with the weights, and neither the journal nor the call policy may treat it as a cloud
    /// one just because its name is not in the list
    pub fn from_config(cfg: &LlmConfig, api_key: Option<String>) -> Result<Self, LlmError> {
        let provider = Provider::parse(&cfg.provider);
        let base_url = if cfg.base_url.trim().is_empty() {
            provider.default_base_url().to_string()
        } else {
            cfg.base_url.clone()
        };
        if base_url.is_empty() {
            return Err(LlmError::Unavailable(
                "the model URL is not set: fill in llm.base_url".into(),
            ));
        }
        let is_local = provider.is_local() || is_loopback_url(&base_url);
        Self::new(
            base_url,
            cfg.model.clone(),
            api_key,
            Duration::from_secs(cfg.timeout_secs),
            provider.id(),
            is_local,
        )
        .map(|client| {
            let client = client
                .with_reasoning_effort(effective_reasoning_effort(&cfg.reasoning_effort, is_local));
            if provider == Provider::OpenRouter {
                client
                    .with_data_collection(Some(cfg.data_collection.clone()))
                    .with_zdr(cfg.zdr)
                    .with_provider_order(provider_slugs(&cfg.provider_order))
                    .with_allow_fallbacks(cfg.allow_fallbacks)
                    .with_require_parameters(cfg.require_parameters)
            } else {
                client
            }
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// Send a request with the key in the header. One place for both calls: the key goes
    /// only into `Authorization`, a timeout is told apart from a server refusal, and the error
    /// message gets the URL but not the key
    fn send(
        &self,
        mut request: reqwest::blocking::RequestBuilder,
        url: &str,
    ) -> Result<reqwest::blocking::Response, LlmError> {
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose());
        }
        request.send().map_err(|err| {
            if err.is_timeout() {
                // A timeout shorter than a second is rounded up in the message, not down to zero
                LlmError::Timeout(self.timeout.as_secs().max(1))
            } else {
                LlmError::Unavailable(format!("{url}: {}", with_causes(&err)))
            }
        })
    }

    /// Names of the models that the server returns on `GET /models`. Diagnostics need it: does
    /// the provider answer at all, and is the configured model downloaded. The errors are the same
    /// as in `complete`
    pub fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.base_url);
        let response = self.send(self.http.get(&url), &url)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(LlmError::Auth);
        }
        if !status.is_success() {
            return Err(LlmError::Unavailable(format!("HTTP {status}")));
        }
        let text = response
            .text()
            .map_err(|err| LlmError::BadResponse(with_causes(&err)))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|err| LlmError::BadResponse(format!("{err}: {}", snippet(&text))))?;
        let names = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .ok_or_else(|| {
                LlmError::BadResponse(format!("the answer has no data list: {}", snippet(&text)))
            })?;
        Ok(names)
    }
}

/// Provider answer: only the fields the journal and the paste need are parsed
#[derive(Debug, Deserialize)]
struct Completion {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    message: Option<String>,
}

/// How many characters of a foreign answer go into the error message
const ERROR_SNIPPET: usize = 200;

fn snippet(text: &str) -> String {
    text.chars().take(ERROR_SNIPPET).collect()
}

/// Error text together with the `source()` chain. reqwest writes only "builder error"
/// or "error sending request" at the top, and keeps the reason that lets the user fix something —
/// no certificates, connection refused — in the source
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

impl LlmClient for OpenAiCompatClient {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn is_local(&self) -> bool {
        self.is_local
    }

    fn complete(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut body = json!({
            "model": if req.model.is_empty() { &self.model } else { &req.model },
            "messages": [
                { "role": "system", "content": req.system },
                { "role": "user", "content": req.user },
            ],
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": false,
        });
        if let Some(effort) = self.reasoning_effort.as_deref() {
            // OpenRouter names the knob `reasoning.effort`; the OpenAI-compatible field is
            // `reasoning_effort`. Sending the wrong one would be silently ignored
            if self.provider_id == "openrouter" {
                // OpenRouter answers HTTP 400 when a model with mandatory reasoning is asked
                // for `none`: the field is left out and the provider default applies
                if effort != "none" {
                    body["reasoning"] = json!({ "effort": effort });
                }
            } else {
                body["reasoning_effort"] = json!(effort);
            }
        }
        if self.provider_id == "openrouter" {
            let mut provider = serde_json::Map::new();
            if let Some(policy) = &self.data_collection {
                provider.insert("data_collection".into(), json!(policy));
            }
            if self.zdr {
                provider.insert("zdr".into(), json!(true));
            }
            // The order is absent when the user did not narrow the choice: an empty array
            // would read as "no provider at all"
            if !self.provider_order.is_empty() {
                provider.insert("order".into(), json!(self.provider_order));
            }
            provider.insert("allow_fallbacks".into(), json!(self.allow_fallbacks));
            provider.insert("require_parameters".into(), json!(self.require_parameters));
            body["provider"] = serde_json::Value::Object(provider);
        }

        let url = self.endpoint();
        let response = self.send(self.http.post(&url).json(&body), &url)?;

        let status = response.status();
        let text = response
            .text()
            .map_err(|err| LlmError::BadResponse(with_causes(&err)))?;

        if status.as_u16() == 401 || status.as_u16() == 403 {
            warn!(provider = %self.provider_id, status = status.as_u16(), "the provider rejected the key");
            return Err(LlmError::Auth);
        }
        if !status.is_success() {
            let parsed: Option<Completion> = serde_json::from_str(&text).ok();
            let detail = parsed
                .and_then(|body| body.error)
                .and_then(|error| error.message)
                .unwrap_or_else(|| snippet(&text));
            return Err(LlmError::Unavailable(format!("HTTP {status}: {detail}")));
        }

        let parsed: Completion = serde_json::from_str(&text)
            .map_err(|err| LlmError::BadResponse(format!("{err}: {}", snippet(&text))))?;
        // OpenRouter can answer HTTP 200 with an error object and an empty choices list. The
        // detailed provider reason is in that object; dropping it leaves "the answer has no
        // text" and hides the hoster's real failure
        if let Some(error) = parsed.error {
            let detail = error.message.unwrap_or_else(|| snippet(&text));
            return Err(LlmError::BadResponse(format!("provider error: {detail}")));
        }
        let content = parsed
            .choices
            .into_iter()
            .find_map(|choice| choice.message.and_then(|message| message.content))
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                LlmError::BadResponse(format!("the answer has no text: {}", snippet(&text)))
            })?;

        let (prompt_tokens, completion_tokens) = match parsed.usage {
            Some(usage) => (usage.prompt_tokens, usage.completion_tokens),
            None => (None, None),
        };
        Ok(ChatResponse {
            text: content,
            prompt_tokens,
            completion_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    /// Local HTTP server with preset answers. There is no outside network
    struct MockServer {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl MockServer {
        /// `None` instead of an answer means "stay silent": the client must hit the timeout
        fn spawn(responses: Vec<Option<String>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port for the mock was found");
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&requests);
            std::thread::spawn(move || {
                for response in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let request = read_request(&mut stream);
                    if let Ok(mut sink) = sink.lock() {
                        sink.push(request);
                    }
                    match response {
                        Some(body) => {
                            let _ = stream.write_all(body.as_bytes());
                            let _ = stream.flush();
                        }
                        None => std::thread::sleep(Duration::from_secs(3)),
                    }
                }
            });
            Self { addr, requests }
        }

        fn base_url(&self) -> String {
            format!("http://{}/v1", self.addr)
        }

        fn last_request(&self) -> String {
            self.requests
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        while let Ok(read) = stream.read(&mut chunk) {
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&buffer).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = content_length(&text[..end]).unwrap_or(0);
                if buffer.len() >= end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buffer).to_string()
    }

    fn content_length(headers: &str) -> Option<usize> {
        headers
            .lines()
            .find(|line| line.to_lowercase().starts_with("content-length:"))
            .and_then(|line| line.split(':').nth(1))
            .and_then(|value| value.trim().parse().ok())
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn ok_body() -> String {
        http(
            "200 OK",
            r#"{"choices":[{"message":{"role":"assistant","content":"Привет, мир."}}],
                "usage":{"prompt_tokens":42,"completion_tokens":7}}"#,
        )
    }

    fn client(server: &MockServer, key: Option<&str>) -> OpenAiCompatClient {
        OpenAiCompatClient::new(
            server.base_url(),
            "qwen3.5:4b",
            key.map(ToString::to_string),
            Duration::from_millis(700),
            "ollama",
            true,
        )
        .unwrap()
    }

    fn request() -> ChatRequest {
        ChatRequest {
            model: String::new(),
            system: "Исправь текст.".into(),
            user: "привет мир".into(),
            temperature: 0.2,
            max_tokens: 256,
        }
    }

    #[test]
    fn a_custom_provider_on_this_machine_is_a_local_model() {
        // "custom" is not a synonym for "remote": a self-hosted endpoint is one process with
        // the weights, and the call policy must not treat it as a cloud provider
        for url in [
            "http://localhost:8000/v1",
            "localhost:8000/v1",
            "http://127.0.0.1:8080/v1",
            "http://127.1.2.3/v1",
            "http://[::1]:11434/v1",
            "https://LOCALHOST/v1",
        ] {
            assert!(is_loopback_url(url), "{url} was taken for a cloud endpoint");
        }
        for url in [
            "https://openrouter.ai/api/v1",
            "https://api.groq.com/openai/v1",
            "http://my-lan-box.local:11434/v1",
            "http://10.0.0.5:1234/v1",
            "https://localhost.example.com/v1",
        ] {
            assert!(!is_loopback_url(url), "{url} was taken for this machine");
        }

        let cfg = LlmConfig {
            provider: "custom".into(),
            base_url: "http://127.0.0.1:8080/v1".into(),
            ..LlmConfig::default()
        };
        let client = OpenAiCompatClient::from_config(&cfg, None).unwrap();
        assert!(client.is_local());
    }

    #[test]
    fn auto_reasoning_effort_silences_local_models_and_leaves_cloud_alone() {
        assert_eq!(
            effective_reasoning_effort("auto", true),
            Some("none".to_string())
        );
        assert_eq!(effective_reasoning_effort("auto", false), None);
        assert_eq!(effective_reasoning_effort("", true), None);
        assert_eq!(
            effective_reasoning_effort("low", false),
            Some("low".to_string())
        );
    }

    #[test]
    fn reasoning_effort_reaches_the_request_body_only_when_set() {
        let server = MockServer::spawn(vec![Some(ok_body()), Some(ok_body())]);
        client(&server, None).complete(&request()).unwrap();
        assert!(
            !server.last_request().contains("reasoning_effort"),
            "without the setting the field is not sent: {}",
            server.last_request()
        );

        client(&server, None)
            .with_reasoning_effort(Some("none".into()))
            .complete(&request())
            .unwrap();
        assert!(
            server
                .last_request()
                .contains(r#""reasoning_effort":"none""#),
            "{}",
            server.last_request()
        );
    }

    #[test]
    fn openrouter_gets_reasoning_effort_in_its_own_shape() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        OpenAiCompatClient::new(
            server.base_url(),
            "z-ai/glm-5.3-flash",
            None,
            Duration::from_millis(700),
            "openrouter",
            false,
        )
        .unwrap()
        .with_reasoning_effort(Some("low".into()))
        .complete(&request())
        .unwrap();
        let sent = server.last_request();
        assert!(sent.contains(r#""reasoning":{"effort":"low"}"#), "{sent}");
        assert!(!sent.contains(r#""reasoning_effort""#), "{sent}");
    }

    #[test]
    fn openrouter_gets_no_reasoning_field_for_none() {
        // A model with mandatory reasoning answers HTTP 400 when it is asked for `none`: the
        // field is left out and the provider default applies
        let server = MockServer::spawn(vec![Some(ok_body())]);
        OpenAiCompatClient::new(
            server.base_url(),
            "z-ai/glm-5.3-flash",
            None,
            Duration::from_millis(700),
            "openrouter",
            false,
        )
        .unwrap()
        .with_reasoning_effort(effective_reasoning_effort("none", false))
        .complete(&request())
        .unwrap();
        let sent = server.last_request();
        assert!(!sent.contains(r#""reasoning""#), "{sent}");
        assert!(!sent.contains("reasoning_effort"), "{sent}");
    }

    #[test]
    fn a_provider_that_is_not_openrouter_still_gets_none() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        client(&server, None)
            .with_reasoning_effort(effective_reasoning_effort("none", true))
            .complete(&request())
            .unwrap();
        assert!(
            server
                .last_request()
                .contains(r#""reasoning_effort":"none""#),
            "{}",
            server.last_request()
        );
    }

    #[test]
    fn openrouter_privacy_fields_share_the_provider_object() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        OpenAiCompatClient::new(
            server.base_url(),
            "z-ai/glm-5.3-flash",
            None,
            Duration::from_millis(700),
            "openrouter",
            false,
        )
        .unwrap()
        .with_data_collection(Some("deny".into()))
        .with_zdr(true)
        .complete(&request())
        .unwrap();
        let sent = server.last_request();
        assert!(sent.contains(r#""data_collection":"deny""#), "{sent}");
        assert!(sent.contains(r#""zdr":true"#), "{sent}");
    }

    #[test]
    fn openrouter_provider_routing_reaches_the_body() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        OpenAiCompatClient::new(
            server.base_url(),
            "z-ai/glm-5.3-flash",
            None,
            Duration::from_millis(700),
            "openrouter",
            false,
        )
        .unwrap()
        .with_provider_order(vec!["groq".into(), "together".into()])
        .with_allow_fallbacks(false)
        .with_require_parameters(true)
        .complete(&request())
        .unwrap();
        let sent = server.last_request();
        assert!(sent.contains(r#""order":["groq","together"]"#), "{sent}");
        assert!(sent.contains(r#""allow_fallbacks":false"#), "{sent}");
        assert!(sent.contains(r#""require_parameters":true"#), "{sent}");
    }

    #[test]
    fn a_config_without_a_hoster_order_sends_no_order_array() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let cfg = LlmConfig {
            provider: "openrouter".into(),
            base_url: server.base_url(),
            ..LlmConfig::default()
        };
        assert_eq!(cfg.provider_order, "");
        OpenAiCompatClient::from_config(&cfg, None)
            .unwrap()
            .complete(&request())
            .unwrap();
        let sent = server.last_request();
        // An empty array would read as "no provider at all" to the router
        assert!(!sent.contains(r#""order""#), "{sent}");
        assert!(sent.contains(r#""allow_fallbacks":true"#), "{sent}");
        assert!(sent.contains(r#""require_parameters":false"#), "{sent}");
    }

    #[test]
    fn the_hoster_order_from_the_config_is_trimmed_lower_case_and_unique() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let cfg = LlmConfig {
            provider: "openrouter".into(),
            base_url: server.base_url(),
            provider_order: " Groq , groq ,Together".into(),
            ..LlmConfig::default()
        };
        OpenAiCompatClient::from_config(&cfg, None)
            .unwrap()
            .complete(&request())
            .unwrap();
        let sent = server.last_request();
        assert!(sent.contains(r#""order":["groq","together"]"#), "{sent}");
    }

    #[test]
    fn a_provider_that_is_not_openrouter_ignores_the_routing_fields() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let cfg = LlmConfig {
            provider: "groq".into(),
            base_url: server.base_url(),
            provider_order: "groq, together".into(),
            allow_fallbacks: false,
            require_parameters: true,
            ..LlmConfig::default()
        };
        OpenAiCompatClient::from_config(&cfg, Some("sk-x".into()))
            .unwrap()
            .complete(&request())
            .unwrap();
        let sent = server.last_request();
        assert!(!sent.contains("\"provider\""), "{sent}");
        assert!(!sent.contains("allow_fallbacks"), "{sent}");
        assert!(!sent.contains("require_parameters"), "{sent}");
    }

    #[test]
    fn a_config_with_auto_effort_and_ollama_sends_none() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let cfg = LlmConfig {
            base_url: server.base_url(),
            provider: "ollama".into(),
            reasoning_effort: "auto".into(),
            ..LlmConfig::default()
        };
        OpenAiCompatClient::from_config(&cfg, None)
            .unwrap()
            .complete(&request())
            .unwrap();
        assert!(server
            .last_request()
            .contains(r#""reasoning_effort":"none""#));
    }

    #[test]
    fn the_default_setting_of_a_local_model_asks_for_no_reasoning() {
        // `none` is the default of the settings: a reasoning model would otherwise spend the
        // whole max_tokens on thinking and answer with an empty text
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let cfg = LlmConfig {
            base_url: server.base_url(),
            provider: "ollama".into(),
            ..LlmConfig::default()
        };
        assert_eq!(cfg.reasoning_effort, "none");
        OpenAiCompatClient::from_config(&cfg, None)
            .unwrap()
            .complete(&request())
            .unwrap();
        assert!(server
            .last_request()
            .contains(r#""reasoning_effort":"none""#));
    }

    #[test]
    fn the_default_setting_of_openrouter_sends_no_reasoning_field() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let cfg = LlmConfig {
            base_url: server.base_url(),
            provider: "openrouter".into(),
            model: "z-ai/glm-5.3-flash".into(),
            ..LlmConfig::default()
        };
        assert_eq!(cfg.reasoning_effort, "none");
        OpenAiCompatClient::from_config(&cfg, None)
            .unwrap()
            .complete(&request())
            .unwrap();
        let sent = server.last_request();
        assert!(!sent.contains("reasoning"), "{sent}");
    }

    #[test]
    fn model_listing_returns_ids_and_reports_a_dead_server() {
        let server = MockServer::spawn(vec![Some(http(
            "200 OK",
            r#"{"object":"list","data":[{"id":"qwen3.5:4b"},{"id":"gemma4:26b"}]}"#,
        ))]);
        let names = client(&server, None).list_models().unwrap();
        assert_eq!(names, vec!["qwen3.5:4b", "gemma4:26b"]);

        let dead = OpenAiCompatClient::new(
            "http://127.0.0.1:9/v1",
            "qwen3.5:4b",
            None,
            Duration::from_secs(2),
            "ollama",
            true,
        )
        .unwrap();
        assert!(matches!(
            dead.list_models().unwrap_err(),
            LlmError::Unavailable(_) | LlmError::Timeout(_)
        ));
    }

    #[test]
    fn a_successful_answer_carries_text_and_token_counts() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let response = client(&server, None).complete(&request()).unwrap();
        assert_eq!(response.text, "Привет, мир.");
        assert_eq!(response.prompt_tokens, Some(42));
        assert_eq!(response.completion_tokens, Some(7));
    }

    #[test]
    fn the_request_is_a_chat_completion_with_both_messages() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        client(&server, None).complete(&request()).unwrap();
        let sent = server.last_request();
        assert!(sent.starts_with("POST /v1/chat/completions "), "{sent}");
        assert!(sent.contains("\"model\":\"qwen3.5:4b\""), "{sent}");
        assert!(sent.contains("\"role\":\"system\""), "{sent}");
        assert!(sent.contains("Исправь текст."), "{sent}");
        assert!(sent.contains("\"role\":\"user\""), "{sent}");
        assert!(sent.contains("привет мир"), "{sent}");
        assert!(sent.contains("\"temperature\":0.2"), "{sent}");
        assert!(sent.contains("\"max_tokens\":256"), "{sent}");
    }

    #[test]
    fn the_key_goes_into_the_header_and_nowhere_else() {
        let secret = "sk-proj-0123456789abcdef";
        let server = MockServer::spawn(vec![Some(ok_body())]);
        let client = client(&server, Some(secret));
        // The key is not in the logs and not in the debug output
        let printed = format!("{client:?}");
        assert!(!printed.contains(secret), "{printed}");
        assert!(printed.contains("sk-…cdef"), "{printed}");

        client.complete(&request()).unwrap();
        let sent = server.last_request();
        assert!(
            sent.contains(&format!("authorization: Bearer {secret}"))
                || sent
                    .to_lowercase()
                    .contains(&format!("authorization: bearer {secret}")),
            "{sent}"
        );
    }

    #[test]
    fn without_a_key_no_authorization_header_is_sent() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        client(&server, None).complete(&request()).unwrap();
        assert!(
            !server
                .last_request()
                .to_lowercase()
                .contains("authorization"),
            "{}",
            server.last_request()
        );
    }

    #[test]
    fn an_empty_key_is_treated_as_no_key() {
        let server = MockServer::spawn(vec![Some(ok_body())]);
        client(&server, Some("   ")).complete(&request()).unwrap();
        assert!(!server
            .last_request()
            .to_lowercase()
            .contains("authorization"));
    }

    #[test]
    fn rejected_credentials_become_an_auth_error() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let server = MockServer::spawn(vec![Some(http(
                status,
                r#"{"error":{"message":"Invalid API key"}}"#,
            ))]);
            let err = client(&server, Some("sk-bad"))
                .complete(&request())
                .unwrap_err();
            assert_eq!(err, LlmError::Auth, "{status}");
        }
    }

    #[test]
    fn a_server_error_becomes_unavailable_with_the_provider_message() {
        let server = MockServer::spawn(vec![Some(http(
            "500 Internal Server Error",
            r#"{"error":{"message":"model is loading"}}"#,
        ))]);
        let err = client(&server, None).complete(&request()).unwrap_err();
        match err {
            LlmError::Unavailable(message) => {
                assert!(message.contains("500"), "{message}");
                assert!(message.contains("model is loading"), "{message}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn a_body_without_text_becomes_a_bad_response() {
        let server = MockServer::spawn(vec![Some(http("200 OK", r#"{"choices":[]}"#))]);
        let err = client(&server, None).complete(&request()).unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");

        let server = MockServer::spawn(vec![Some(http("200 OK", "this is not json"))]);
        let err = client(&server, None).complete(&request()).unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[test]
    fn a_silent_server_becomes_a_timeout_with_the_configured_seconds() {
        let server = MockServer::spawn(vec![None]);
        let client = OpenAiCompatClient::new(
            server.base_url(),
            "m",
            None,
            Duration::from_millis(150),
            "ollama",
            true,
        )
        .unwrap();
        let err = client.complete(&request()).unwrap_err();
        // A timeout of 150 ms is rounded up to a second in the message, not down to zero
        assert_eq!(err, LlmError::Timeout(1), "{err:?}");
    }

    #[test]
    fn a_closed_port_becomes_unavailable_not_a_panic() {
        // The port is taken and released at once, so nothing listens on the address
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        // Windows does not answer a closed port at once, but after several SYN retries (about a
        // second). The timeout must be clearly larger, or the test gets "timeout" instead of
        // "unavailable"
        let client = OpenAiCompatClient::new(
            format!("http://{addr}/v1"),
            "m",
            None,
            Duration::from_secs(10),
            "ollama",
            true,
        )
        .unwrap();
        let err = client.complete(&request()).unwrap_err();
        assert!(matches!(err, LlmError::Unavailable(_)), "{err:?}");
    }

    #[test]
    fn provider_presets_know_their_address_and_locality() {
        assert_eq!(Provider::parse("ollama"), Provider::Ollama);
        assert_eq!(Provider::parse("LM-Studio"), Provider::LmStudio);
        assert_eq!(Provider::parse("something custom"), Provider::Custom);
        assert!(Provider::Ollama.is_local());
        assert!(Provider::LmStudio.is_local());
        assert!(!Provider::Groq.is_local());
        assert!(!Provider::OpenAi.is_local());
        assert_eq!(
            Provider::Groq.default_base_url(),
            "https://api.groq.com/openai/v1"
        );
        assert_eq!(Provider::Custom.default_base_url(), "");
    }

    #[test]
    fn a_client_from_config_uses_the_preset_when_the_address_is_empty() {
        let cfg = LlmConfig {
            provider: "groq".into(),
            base_url: String::new(),
            ..LlmConfig::default()
        };
        let client = OpenAiCompatClient::from_config(&cfg, Some("sk-x".into())).unwrap();
        assert_eq!(client.base_url(), "https://api.groq.com/openai/v1");
        assert_eq!(client.id(), "groq");
        assert!(!client.is_local());
        assert_eq!(
            client.endpoint(),
            "https://api.groq.com/openai/v1/chat/completions"
        );
    }

    #[test]
    fn a_custom_provider_without_an_address_refuses_to_start() {
        let cfg = LlmConfig {
            provider: "custom".into(),
            base_url: String::new(),
            ..LlmConfig::default()
        };
        let err = OpenAiCompatClient::from_config(&cfg, None).unwrap_err();
        assert!(matches!(err, LlmError::Unavailable(_)), "{err:?}");
    }

    #[test]
    fn a_trailing_slash_in_the_address_does_not_double_up() {
        let client = OpenAiCompatClient::new(
            "http://localhost:11434/v1/",
            "m",
            None,
            Duration::from_secs(1),
            "ollama",
            true,
        )
        .unwrap();
        assert_eq!(
            client.endpoint(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[derive(Debug, thiserror::Error)]
    #[error("builder error")]
    struct Outer(#[source] Inner);

    #[derive(Debug, thiserror::Error)]
    #[error("no CA certificates found")]
    struct Inner;

    #[test]
    fn an_error_message_keeps_the_cause_reqwest_hides_behind_its_own() {
        let text = with_causes(&Outer(Inner));
        assert!(text.starts_with("builder error"), "{text}");
        assert!(text.contains("no CA certificates found"), "{text}");
    }

    #[test]
    fn an_error_without_a_cause_reads_as_itself() {
        assert_eq!(with_causes(&Inner), "no CA certificates found");
    }
}
