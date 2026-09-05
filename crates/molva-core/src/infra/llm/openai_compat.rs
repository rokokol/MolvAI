// SPDX-License-Identifier: MIT
//! Клиент `/chat/completions` в диалекте OpenAI: Ollama, LM Studio, OpenRouter, Groq, OpenAI.
//!
//! Запрос синхронный (`reqwest::blocking`): в ядре нет асинхронной среды, а реплика всё равно
//! ждёт ответа модели. Ретраи живут в конвейере, здесь одна попытка — так таймаут означает
//! ровно то, что настроено, а не «таймаут, умноженный на число попыток».
//!
//! Ключ хранится в [`ApiKey`], поэтому `Debug` клиента печатает маску, а не секрет.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::app::secrets::ApiKey;
use crate::config::LlmConfig;
use crate::domain::llm::{ChatRequest, ChatResponse, LlmClient, LlmError};

/// Известные провайдеры: пресеты базового адреса и признака локальности.
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
    /// Разбор значения `llm.provider`; неизвестное имя — `Custom`, а не ошибка.
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

    /// Адрес по умолчанию; для `Custom` пусто — его обязан задать пользователь.
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

    /// Локальная ли модель: попадает в журнал полем `local_llm` (критерий S-08).
    pub fn is_local(&self) -> bool {
        matches!(self, Provider::Ollama | Provider::LmStudio)
    }
}

/// Клиент одного провайдера.
pub struct OpenAiCompatClient {
    base_url: String,
    model: String,
    api_key: Option<ApiKey>,
    provider_id: String,
    is_local: bool,
    timeout: Duration,
    http: reqwest::blocking::Client,
}

impl fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCompatClient")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("is_local", &self.is_local)
            .field("api_key", &self.api_key)
            .finish()
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
            .map_err(|e| LlmError::Unavailable(e.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key: api_key.map(ApiKey::new).filter(|key| !key.is_empty()),
            provider_id: provider_id.into(),
            is_local,
            timeout,
            http,
        })
    }

    /// Клиент по настройкам: пустой `base_url` берётся из пресета провайдера.
    pub fn from_config(cfg: &LlmConfig, api_key: Option<String>) -> Result<Self, LlmError> {
        let provider = Provider::parse(&cfg.provider);
        let base_url = if cfg.base_url.trim().is_empty() {
            provider.default_base_url().to_string()
        } else {
            cfg.base_url.clone()
        };
        if base_url.is_empty() {
            return Err(LlmError::Unavailable(
                "не задан адрес модели: заполните llm.base_url".into(),
            ));
        }
        Self::new(
            base_url,
            cfg.model.clone(),
            api_key,
            Duration::from_secs(cfg.timeout_secs),
            provider.id(),
            provider.is_local(),
        )
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
}

/// Ответ провайдера: разбираем только то, что нужно журналу и вставке.
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

/// Сколько символов чужого ответа попадает в сообщение об ошибке.
const ERROR_SNIPPET: usize = 200;

fn snippet(text: &str) -> String {
    text.chars().take(ERROR_SNIPPET).collect()
}

impl LlmClient for OpenAiCompatClient {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn is_local(&self) -> bool {
        self.is_local
    }

    fn complete(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = json!({
            "model": if req.model.is_empty() { &self.model } else { &req.model },
            "messages": [
                { "role": "system", "content": req.system },
                { "role": "user", "content": req.user },
            ],
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": false,
        });

        let mut request = self.http.post(self.endpoint()).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose());
        }

        let response = request.send().map_err(|err| {
            if err.is_timeout() {
                LlmError::Timeout(self.timeout.as_secs())
            } else {
                // В сообщение попадает адрес, но не ключ: он живёт только в заголовке.
                LlmError::Unavailable(format!("{}: {err}", self.endpoint()))
            }
        })?;

        let status = response.status();
        let text = response
            .text()
            .map_err(|err| LlmError::BadResponse(err.to_string()))?;

        if status.as_u16() == 401 || status.as_u16() == 403 {
            warn!(provider = %self.provider_id, status = status.as_u16(), "провайдер отверг ключ");
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
        let content = parsed
            .choices
            .into_iter()
            .find_map(|choice| choice.message.and_then(|message| message.content))
            .ok_or_else(|| {
                LlmError::BadResponse(format!("в ответе нет текста: {}", snippet(&text)))
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

    /// Локальный HTTP-сервер на заранее заготовленных ответах. Сети наружу нет.
    struct MockServer {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl MockServer {
        /// `None` вместо ответа означает «молчим»: клиент должен упереться в таймаут.
        fn spawn(responses: Vec<Option<String>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("порт для мока нашёлся");
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
            key.map(|k| k.to_string()),
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
        // В логах и отладочном выводе ключа нет.
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
            other => panic!("ожидали Unavailable, получили {other:?}"),
        }
    }

    #[test]
    fn a_body_without_text_becomes_a_bad_response() {
        let server = MockServer::spawn(vec![Some(http("200 OK", r#"{"choices":[]}"#))]);
        let err = client(&server, None).complete(&request()).unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");

        let server = MockServer::spawn(vec![Some(http("200 OK", "это не json"))]);
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
        assert_eq!(err, LlmError::Timeout(0), "{err:?}");
    }

    #[test]
    fn a_closed_port_becomes_unavailable_not_a_panic() {
        // Порт занимаем и сразу освобождаем: адрес заведомо никем не слушается.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let client = OpenAiCompatClient::new(
            format!("http://{addr}/v1"),
            "m",
            None,
            Duration::from_millis(500),
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
        assert_eq!(Provider::parse("что-то своё"), Provider::Custom);
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
}
