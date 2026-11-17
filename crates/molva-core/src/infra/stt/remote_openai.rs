// SPDX-License-Identifier: MIT
//! Remote recognition over the OpenAI `/audio/transcriptions` dialect: OpenAI, Groq, OpenRouter
//!
//! One `POST` per phrase: the audio goes as a 16 kHz mono WAV, the answer is a JSON transcription
//! with segments and, when the provider accounts, the price of the call
//!
//! The multipart body is built here, byte by byte, instead of through the `multipart` feature of
//! reqwest: the body has five fields of known shape, and a second HTTP stack for them is not worth
//! a build dependency

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use tracing::warn;

use crate::app::secrets::ApiKey;
use crate::config::RemoteSttConfig;
use crate::domain::audio::{PcmAudio, TARGET_SAMPLE_RATE};
use crate::domain::cancel::CancelToken;
use crate::domain::stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};
use crate::infra::http_policy::{with_causes, HttpPolicy};

/// Path of the transcription endpoint under the base URL of the settings
const TRANSCRIPTIONS_PATH: &str = "audio/transcriptions";

/// Name, file name and type of the audio part: what every OpenAI-compatible server expects
const AUDIO_FIELD: &str = "file";
const AUDIO_FILENAME: &str = "audio.wav";
const AUDIO_MIME: &str = "audio/wav";

/// Segments and language need the verbose form; the plain one carries the text alone
const RESPONSE_FORMAT: &str = "verbose_json";

/// How many characters of a foreign answer go into the error message
const ERROR_SNIPPET: usize = 200;

/// The floor and the ceiling of one request, in seconds
///
/// The floor covers the upload and the answer for a long phrase; the ceiling keeps a provider that
/// stopped answering from holding the recognizer, and the daemon, for ever
pub const MIN_TIMEOUT_SECS: u64 = 5;
pub const MAX_TIMEOUT_SECS: u64 = 900;

/// How many more requests the engine repeats at most: a provider that is down stays down, and a
/// phrase must not wait through an endless chain of attempts
pub const MAX_RETRIES: u32 = 5;

/// Pause before the second attempt, multiplied by the number of the attempt: a rate limit eases
/// while the engine waits
const RETRY_PAUSE: Duration = Duration::from_millis(300);

/// One attempt and how the next one should treat it
enum Attempt {
    /// The provider answered something that a repeat will not change
    Fatal(SttError),
    /// A timeout, a rate limit or a provider error: the same request may succeed
    Transient(SttError),
}

/// How a request is routed through OpenRouter
///
/// These are the body fields the service reads: a provider of its own understands none of them,
/// and a request to one of those carries no `provider` object at all
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouterPolicy {
    data_collection: String,
    zdr: bool,
    /// Comma-separated provider slugs, empty when the router picks
    order: String,
    allow_fallbacks: bool,
}

impl RouterPolicy {
    /// The `provider` part of the request, as the service parses it
    fn to_json(&self) -> String {
        let order: Vec<&str> = self
            .order
            .split(',')
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .collect();
        let mut routing = serde_json::Map::new();
        routing.insert(
            "data_collection".into(),
            self.data_collection.clone().into(),
        );
        routing.insert("zdr".into(), self.zdr.into());
        routing.insert("allow_fallbacks".into(), self.allow_fallbacks.into());
        if !order.is_empty() {
            routing.insert("order".into(), serde_json::json!(order));
        }
        serde_json::Value::Object(routing).to_string()
    }
}

/// Recognizer on a remote OpenAI-compatible API
pub struct RemoteOpenAiEngine {
    base_url: String,
    model: String,
    api_key: ApiKey,
    timeout: Duration,
    /// Sampling temperature of the provider; 0 keeps the reading of a phrase the same every time
    temperature: f32,
    /// Words the model should lean towards; empty sends no prompt at all
    prompt: String,
    /// How many more requests follow a timeout, a rate limit or a provider error
    max_retries: u32,
    /// Routing of the request through OpenRouter; `None` for a service of its own, which takes
    /// none of those fields
    router: Option<RouterPolicy>,
    policy: HttpPolicy,
    http: reqwest::blocking::Client,
    /// The job a client may give up on; checked before the send and after the answer
    cancel: Option<std::sync::Arc<CancelToken>>,
}

impl fmt::Debug for RemoteOpenAiEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteOpenAiEngine")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
}

impl RemoteOpenAiEngine {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
        cloud_consent: bool,
    ) -> Result<Self, SttError> {
        let policy = HttpPolicy::new(cloud_consent);
        let http = policy
            .client(timeout)
            .map_err(|e| SttError::Inference(format!("cannot build the HTTP client: {e}")))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key: ApiKey::new(api_key),
            timeout,
            temperature: 0.0,
            prompt: String::new(),
            max_retries: 0,
            router: None,
            policy,
            http,
            cancel: None,
        })
    }

    /// Route the request through OpenRouter with these preferences
    #[must_use]
    pub(crate) fn with_router(mut self, router: RouterPolicy) -> Self {
        self.router = Some(router);
        self
    }

    /// Temperature of the decoder: 0 is greedy decoding, which is what a dictated phrase wants
    #[must_use]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature.clamp(0.0, 1.0);
        self
    }

    /// Words the model should lean towards: names, jargon and the spelling of the dictionary
    #[must_use]
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// How many more requests follow one that failed on a timeout, a rate limit or a 5xx
    #[must_use]
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries.min(MAX_RETRIES);
        self
    }

    /// Client from the settings: the address, the model and the timeout come from
    /// `stt.remote`, the key from the key store or the environment
    pub fn from_remote(
        remote: &RemoteSttConfig,
        api_key: &str,
        cloud_consent: bool,
    ) -> Result<Self, SttError> {
        let seconds = u64::from(remote.timeout_secs).clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        Self::new(
            &remote.base_url,
            &remote.model,
            api_key,
            Duration::from_secs(seconds),
            cloud_consent,
        )
        .map(|engine| {
            let engine = engine
                .with_temperature(remote.temperature)
                .with_prompt(remote.prompt.clone())
                .with_retries(remote.max_retries);
            // The routing and the terms are OpenRouter's own: another service takes a prompt and
            // would reject a part it does not know
            if Self::is_openrouter_address(&remote.base_url) {
                engine.with_router(RouterPolicy {
                    data_collection: remote.data_collection.clone(),
                    zdr: remote.zdr,
                    order: remote.provider_order.clone(),
                    allow_fallbacks: remote.allow_fallbacks,
                })
            } else {
                engine
            }
        })
    }

    /// `{base_url}/audio/transcriptions`
    fn endpoint(&self) -> String {
        format!("{}/{TRANSCRIPTIONS_PATH}", self.base_url)
    }

    /// Whether the address belongs to OpenRouter
    ///
    /// The fields of `provider` and `keyterms` are that service's own: OpenAI and Groq take a
    /// `prompt` and no routing at all, and a request must not carry what the other side will reject
    fn is_openrouter_address(base_url: &str) -> bool {
        reqwest::Url::parse(base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .is_some_and(|host| host == "openrouter.ai" || host.ends_with(".openrouter.ai"))
    }

    /// Whether the client gave up before this call reached the provider
    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    /// One request, classified by what a repeat of it would mean
    fn attempt(&self, url: &str, wav: &[u8], options: &SttOptions) -> Result<Transcript, Attempt> {
        let (content_type, body) = multipart_body(
            &self.model,
            language_field(options),
            self.temperature,
            &self.prompt,
            self.router.as_ref(),
            wav,
        );
        let response = self
            .http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .bearer_auth(self.api_key.expose())
            .body(body)
            .send()
            .map_err(|err| {
                let error = transport_error(&err, url, self.timeout);
                // A timeout and a connection that dropped may pass on the next try; a request the
                // client itself refused never will
                if err.is_timeout() || err.is_connect() || err.is_request() {
                    Attempt::Transient(error)
                } else {
                    Attempt::Fatal(error)
                }
            })?;
        if self.policy.guard_response(&response).is_err() {
            return Err(Attempt::Fatal(SttError::Inference(
                "the provider answered with a redirect, and remote recognition follows none".into(),
            )));
        }
        let status = response.status();
        let text = response.text().map_err(|err| {
            Attempt::Fatal(SttError::Inference(format!(
                "cannot read the answer of the provider: {}",
                with_causes(&err)
            )))
        })?;
        if !status.is_success() {
            let error = provider_error(status, &text);
            // A rate limit and a provider error of its own side pass; a key or a model the provider
            // refuses does not
            return Err(
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    Attempt::Transient(error)
                } else {
                    Attempt::Fatal(error)
                },
            );
        }
        let transcript = parse_transcript(&text).map_err(Attempt::Fatal)?;
        if self.is_cancelled() {
            return Err(Attempt::Fatal(SttError::Cancelled));
        }
        Ok(transcript)
    }

    /// Wait between two attempts in slices, so a cancelled job leaves at once
    ///
    /// Returns `true` when the client cancelled during the wait
    fn pause(&self, total: Duration) -> bool {
        const SLICE: Duration = Duration::from_millis(50);
        let mut left = total;
        while left > Duration::ZERO {
            if self.is_cancelled() {
                return true;
            }
            let step = left.min(SLICE);
            std::thread::sleep(step);
            left -= step;
        }
        self.is_cancelled()
    }
}

/// The audio as a 16 kHz mono 16-bit WAV, in memory
///
/// The bytes go into one request and nothing else reads them, so there is no temporary file to
/// create, to clean up and to leak when the process dies in the middle of a phrase
fn wav_bytes(audio: &PcmAudio) -> Result<Vec<u8>, SttError> {
    let ready = audio.to_16k();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav: Vec<u8> = Vec::with_capacity(ready.samples.len() * 2 + 64);
    {
        let mut writer = hound::WavWriter::new(std::io::Cursor::new(&mut wav), spec)
            .map_err(|e| SttError::Inference(format!("cannot write the WAV header: {e}")))?;
        for sample in &ready.samples {
            writer
                .write_sample(to_i16(*sample))
                .map_err(|e| SttError::Inference(format!("cannot write the WAV data: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| SttError::Inference(format!("cannot finish the WAV: {e}")))?;
    }
    Ok(wav)
}

/// 16-bit sample of a float one
fn to_i16(sample: f32) -> i16 {
    // A sample outside the range would wrap into the opposite sign: clamping is what a decoder of
    // a broken file expects, and the output of a recognizer already stays inside
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

/// Body of the request and the value of its `Content-Type`
fn multipart_body(
    model: &str,
    language: Option<&str>,
    temperature: f32,
    prompt: &str,
    router: Option<&RouterPolicy>,
    wav: &[u8],
) -> (String, Vec<u8>) {
    let boundary = boundary_for(wav);
    let mut body = Vec::with_capacity(wav.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{AUDIO_FIELD}\"; \
             filename=\"{AUDIO_FILENAME}\"\r\nContent-Type: {AUDIO_MIME}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    text_part(&mut body, &boundary, "model", model);
    if let Some(language) = language {
        text_part(&mut body, &boundary, "language", language);
    }
    text_part(&mut body, &boundary, "response_format", RESPONSE_FORMAT);
    // Greedy decoding is the default of a dictated phrase: the same phrase must read the same way
    // every time it is sent
    text_part(
        &mut body,
        &boundary,
        "temperature",
        &format_temperature(temperature),
    );
    let prompt = prompt.trim();
    match router {
        // OpenRouter biases the decoder with separate terms, and takes the routing in `provider`
        Some(router) => {
            text_part(&mut body, &boundary, "provider", &router.to_json());
            for term in keyterms(prompt) {
                text_part(&mut body, &boundary, "keyterms[]", &term);
            }
        }
        // The other services take one prompt and no routing at all
        None if !prompt.is_empty() => text_part(&mut body, &boundary, "prompt", prompt),
        None => {}
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// The terms of a hint, one by one: OpenRouter takes a list, so the line is split on its commas
fn keyterms(prompt: &str) -> Vec<String> {
    prompt
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect()
}

/// The temperature as the provider parses it: `0` rather than `0.000000` for the default
fn format_temperature(temperature: f32) -> String {
    let value = f64::from(temperature.clamp(0.0, 1.0));
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn text_part(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

/// Boundary that no byte of the audio contains
///
/// Every byte of the audio is data, and a boundary that happens to appear inside it would split a
/// part in the middle of the recording. There is no random source in the core, so the search starts
/// from the process and the attempt that no recording contains
fn boundary_for(audio: &[u8]) -> String {
    let mut attempt = 0u32;
    loop {
        let candidate = format!("----molva{}-{attempt}", std::process::id());
        let bytes = candidate.as_bytes();
        if !audio.windows(bytes.len()).any(|window| window == bytes) {
            return candidate;
        }
        attempt = attempt.saturating_add(1);
    }
}

/// Language of the request: a fixed one is sent, `auto` is left out so that the model chooses
fn language_field(options: &SttOptions) -> Option<&str> {
    match &options.language {
        LanguageHint::Fixed(code) if !code.trim().is_empty() => Some(code.as_str()),
        _ => None,
    }
}

/// Provider answer: the fields the journal and the caller need, and nothing else
#[derive(Debug, Deserialize)]
struct Transcription {
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Vec<WireSegment>,
    #[serde(default)]
    language: Option<String>,
    /// Length of the audio as the provider measured it
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct WireSegment {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    /// Seconds of audio the provider billed for; Groq reports the shape `{"type":"duration"}`
    #[serde(default)]
    seconds: Option<f64>,
    /// US dollars. OpenRouter reports it on every answer, OpenAI and Groq do not
    #[serde(default)]
    cost: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    message: Option<String>,
}

fn snippet(text: &str) -> String {
    text.chars().take(ERROR_SNIPPET).collect()
}

/// Transcript of a successful answer
fn parse_transcript(text: &str) -> Result<Transcript, SttError> {
    let parsed: Transcription = serde_json::from_str(text).map_err(|err| {
        SttError::Inference(format!(
            "the answer is not a transcription: {err}: {}",
            snippet(text)
        ))
    })?;
    let cost = parsed.usage.as_ref().and_then(|usage| usage.cost);
    let billed = parsed
        .usage
        .as_ref()
        .and_then(|usage| usage.seconds)
        .or(parsed.duration);
    Ok(Transcript {
        text: parsed.text,
        segments: parsed
            .segments
            .iter()
            .map(|segment| Segment {
                start_ms: seconds_to_ms(segment.start),
                end_ms: seconds_to_ms(segment.end),
                text: segment.text.clone(),
            })
            .collect(),
        detected_language: language_code(parsed.language.as_deref()),
        no_speech_prob: None,
        cost_usd: cost,
        billed_seconds: billed,
    })
}

/// Milliseconds of a moment that the provider gives in seconds
fn seconds_to_ms(seconds: f64) -> u32 {
    let ms = (seconds * 1000.0).round();
    if !ms.is_finite() || ms <= 0.0 {
        return 0;
    }
    ms.min(f64::from(u32::MAX)) as u32
}

/// Language in the form the journal keeps it
///
/// Whisper-based providers name the language in words (`russian`), and a word is no ISO-639-1
/// code. A short answer is taken as a code; a name gives `None`, because the pipeline compares
/// this value against the list of allowed codes, and a name there would buy an extra paid request
/// on every phrase
fn language_code(language: Option<&str>) -> Option<String> {
    let language = language?.trim().to_lowercase();
    let is_code = (2..=3).contains(&language.chars().count())
        && language.chars().all(|c| c.is_ascii_alphabetic());
    is_code.then_some(language)
}

/// Error of a failed request: the address, the status and the reason of the provider, never the key
fn transport_error(error: &reqwest::Error, url: &str, timeout: Duration) -> SttError {
    if error.is_timeout() {
        // A timeout shorter than a second is rounded up in the message, not down to zero
        let seconds = timeout.as_secs().max(1);
        return SttError::Inference(format!(
            "no answer from the recognition provider within {seconds} s"
        ));
    }
    SttError::Inference(format!("{url}: {}", with_causes(error)))
}

/// Error of an answer that is not a success
fn provider_error(status: reqwest::StatusCode, text: &str) -> SttError {
    let detail = serde_json::from_str::<ApiError>(text)
        .ok()
        .and_then(|body| body.error)
        .and_then(|error| error.message)
        .unwrap_or_else(|| snippet(text));
    if status == reqwest::StatusCode::UNAUTHORIZED {
        // The key travels in the header alone, and this message reaches the journal and the log
        warn!(
            status = status.as_u16(),
            "the recognition provider rejected the key"
        );
        return SttError::Inference(format!(
            "the recognition provider rejected the key (HTTP {status}): {detail}"
        ));
    }
    SttError::Inference(format!(
        "the recognition provider answered HTTP {status}: {detail}"
    ))
}

impl SttEngine for RemoteOpenAiEngine {
    fn id(&self) -> &'static str {
        "remote-openai"
    }

    fn model_name(&self) -> &'static str {
        "remote-openai"
    }

    /// One request per phrase
    ///
    /// A request that is already on the wire cannot be recalled: the cancel is honoured before
    /// the send and after the answer, so a job the client gave up on is dropped here at the first
    /// of those two moments. The audio of a request cancelled in flight is still delivered, and
    /// the provider still bills it
    fn transcribe(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
    ) -> Result<Transcript, SttError> {
        if self.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        if audio.samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        let url = self.endpoint();
        if self.policy.guard(&url).is_err() {
            return Err(SttError::Inference(
                "remote recognition would leave this machine: enable privacy.cloud_consent".into(),
            ));
        }
        let wav = wav_bytes(audio)?;
        let attempts = self.max_retries.saturating_add(1);
        let mut last: Option<SttError> = None;
        for attempt in 0..attempts {
            if attempt > 0 && self.pause(RETRY_PAUSE * attempt) {
                return Err(SttError::Cancelled);
            }
            match self.attempt(&url, &wav, options) {
                Ok(transcript) => return Ok(transcript),
                Err(Attempt::Fatal(error)) => return Err(error),
                Err(Attempt::Transient(error)) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| {
            SttError::Inference("the request was not sent: no attempt was made".into())
        }))
    }

    /// Nothing to free: the weights are the provider's
    fn unload(&mut self) {}

    fn set_cancel(&mut self, cancel: Option<std::sync::Arc<CancelToken>>) {
        self.cancel = cancel;
    }

    /// Another request in flight: the settings are the same, and the job token is not, because a
    /// replica answers one chunk and the runner cancels it by not sending the next one
    fn replica(&self) -> Option<Box<dyn SttEngine>> {
        let http = self.policy.client(self.timeout).ok()?;
        Some(Box::new(Self {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: ApiKey::new(self.api_key.expose()),
            timeout: self.timeout,
            temperature: self.temperature,
            prompt: self.prompt.clone(),
            max_retries: self.max_retries,
            router: self.router.clone(),
            policy: self.policy,
            http,
            cancel: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use crate::domain::audio::PcmAudio;
    use crate::domain::stt::{LanguageHint, Segment, SttError, SttOptions};

    /// Local HTTP server with preset answers, and the bytes of every request it received
    ///
    /// `None` instead of an answer means "stay silent": the client must hit the timeout
    struct MockServer {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl MockServer {
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

        /// The same server under an address that is not this machine: the policy refuses it
        /// without consent, and the connection still works when it allows it
        fn remote_base_url(&self) -> String {
            format!("http://0.0.0.0:{}/v1", self.addr.port())
        }

        fn last(&self) -> Vec<u8> {
            self.requests
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
        }

        fn calls(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(read) = stream.read(&mut chunk) {
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(end) = find(&buffer, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buffer[..end]).to_string();
                let length = header(&head, "content-length")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if buffer.len() >= end + 4 + length {
                    break;
                }
            }
        }
        buffer
    }

    fn head_of(request: &[u8]) -> String {
        let end = find(request, b"\r\n\r\n").expect("the request has a head");
        String::from_utf8_lossy(&request[..end]).to_string()
    }

    fn body_of(request: &[u8]) -> Vec<u8> {
        let end = find(request, b"\r\n\r\n").expect("the request has a head") + 4;
        request[end..].to_vec()
    }

    fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    fn boundary(head: &str) -> String {
        let content_type = header(head, "content-type").expect("a content type");
        content_type
            .split("boundary=")
            .nth(1)
            .expect("a boundary in the content type")
            .to_string()
    }

    /// The bytes of the audio part alone, without its headers
    fn audio_part(body: &[u8]) -> Vec<u8> {
        const MARKER: &[u8] = b"Content-Disposition: form-data; name=\"file\"; \
            filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n";
        let start = find(body, MARKER).expect("the file part") + MARKER.len();
        let end = find(&body[start..], b"\r\n--").expect("the end of the file part");
        body[start..start + end].to_vec()
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn verbose_json() -> String {
        http(
            "200 OK",
            r#"{"text":"Привет, мир.","language":"russian","duration":2.5,
                "segments":[{"id":0,"start":0.0,"end":1.25,"text":"Привет,"},
                            {"id":1,"start":1.25,"end":2.5,"text":"мир."}]}"#,
        )
    }

    fn engine_at(base_url: &str, cloud_consent: bool, timeout: Duration) -> RemoteOpenAiEngine {
        RemoteOpenAiEngine::new(
            base_url,
            "whisper-large-v3-turbo",
            "sk-test-0123456789abcdef",
            timeout,
            cloud_consent,
        )
        .unwrap()
    }

    fn audio() -> PcmAudio {
        PcmAudio::new(vec![0.5; 1600], 16_000)
    }

    fn russian() -> SttOptions {
        SttOptions {
            language: LanguageHint::Fixed("ru".into()),
            ..SttOptions::default()
        }
    }

    #[test]
    fn the_request_is_a_multipart_form_with_the_documented_fields() {
        let server = MockServer::spawn(vec![Some(verbose_json()), Some(verbose_json())]);
        engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap();

        let request = server.last();
        let head = head_of(&request);
        assert!(
            head.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"),
            "{head}"
        );
        assert_eq!(
            header(&head, "authorization"),
            Some("Bearer sk-test-0123456789abcdef"),
            "{head}"
        );
        let boundary = boundary(&head);
        assert!(!boundary.is_empty(), "{head}");
        assert_eq!(
            body_of(&request),
            expected_body(&boundary, &canonical_wav(1600)),
            "the multipart body is the wire format another program parses"
        );

        // The language is a hint: without one there is no field at all, and the model chooses
        engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &SttOptions::default())
            .unwrap();
        let sent = body_of(&server.last());
        assert!(
            find(&sent, b"name=\"language\"").is_none(),
            "{}",
            String::from_utf8_lossy(&sent)
        );
    }

    /// The body a server must see, written out from the standard rather than from the client
    fn expected_body(boundary: &str, wav: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
                 filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(wav);
        body.extend_from_slice(b"\r\n");
        for (name, value) in [
            ("model", "whisper-large-v3-turbo"),
            ("language", "ru"),
            ("response_format", "verbose_json"),
            ("temperature", "0"),
        ] {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        body
    }

    /// A WAV of a constant sample, byte by byte: 44 bytes of header, then 16-bit little endian
    fn canonical_wav(samples: usize) -> Vec<u8> {
        let data = samples * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&32_000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data as u32).to_le_bytes());
        // 0.5 of full scale is 16383 after the rounding of a 16-bit sample
        for _ in 0..samples {
            wav.extend_from_slice(&16_384u16.to_le_bytes());
        }
        wav
    }

    fn le_u16(bytes: &[u8]) -> u16 {
        u16::from_le_bytes([bytes[0], bytes[1]])
    }

    fn le_u32(bytes: &[u8]) -> u32 {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    #[test]
    fn the_audio_part_is_a_16_khz_mono_16_bit_wav() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        // A phrase recorded at 48 kHz goes to the provider at 16 kHz, not as it came
        engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(
                &PcmAudio::new(vec![0.25; 4800], 48_000),
                &SttOptions::default(),
            )
            .unwrap();
        let wav = audio_part(&body_of(&server.last()));
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(le_u16(&wav[20..22]), 1, "PCM");
        assert_eq!(le_u16(&wav[22..24]), 1, "mono");
        assert_eq!(le_u32(&wav[24..28]), 16_000, "sample rate");
        assert_eq!(le_u16(&wav[34..36]), 16, "bits per sample");
        assert_eq!(&wav[36..40], b"data");
        // 4800 samples at 48 kHz are 1600 at 16 kHz, two bytes each
        assert_eq!(le_u32(&wav[40..44]), 3200);
        assert_eq!(wav.len(), 44 + 3200);
    }

    #[test]
    fn a_verbose_answer_gives_text_segments_and_milliseconds() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let transcript = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap();
        assert_eq!(transcript.text, "Привет, мир.");
        assert_eq!(
            transcript.segments,
            vec![
                Segment {
                    start_ms: 0,
                    end_ms: 1250,
                    text: "Привет,".into(),
                },
                Segment {
                    start_ms: 1250,
                    end_ms: 2500,
                    text: "мир.".into(),
                },
            ]
        );
        // The provider names the language in words: a word is no ISO-639-1 code, and a code the
        // journal cannot use is worse than no answer
        assert_eq!(transcript.detected_language, None);
    }

    #[test]
    fn a_language_that_is_already_a_code_reaches_the_journal() {
        let server = MockServer::spawn(vec![Some(http(
            "200 OK",
            r#"{"text":"hello","language":"EN"}"#,
        ))]);
        let transcript = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap();
        assert_eq!(transcript.detected_language.as_deref(), Some("en"));
    }

    #[test]
    fn a_provider_that_reports_a_price_fills_the_cost_and_the_billed_seconds() {
        let server = MockServer::spawn(vec![Some(http(
            "200 OK",
            r#"{"text":"ok","usage":{"type":"duration","seconds":2.5,"cost":0.00005}}"#,
        ))]);
        let transcript = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap();
        assert_eq!(transcript.cost_usd, Some(0.000_05));
        assert_eq!(transcript.billed_seconds, Some(2.5));
    }

    #[test]
    fn a_provider_without_a_price_leaves_the_cost_unknown() {
        // A duration is not a price: the table of prices of no provider is filled in here
        let server = MockServer::spawn(vec![Some(http(
            "200 OK",
            r#"{"text":"ok","duration":4.0}"#,
        ))]);
        let transcript = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap();
        assert_eq!(transcript.cost_usd, None);
        assert_eq!(transcript.billed_seconds, Some(4.0));
    }

    #[test]
    fn a_rejected_key_is_reported_and_the_key_stays_out_of_the_message() {
        let server = MockServer::spawn(vec![Some(http(
            "401 Unauthorized",
            r#"{"error":{"message":"Invalid API key"}}"#,
        ))]);
        let error = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap_err()
            .to_string();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("Invalid API key"), "{error}");
        assert!(!error.contains("sk-test-0123456789abcdef"), "{error}");
    }

    #[test]
    fn a_rate_limit_and_a_server_error_keep_the_reason_of_the_provider() {
        for (status, code, reason) in [
            ("429 Too Many Requests", "429", "rate limit exceeded"),
            ("500 Internal Server Error", "500", "model is loading"),
        ] {
            let body = format!(r#"{{"error":{{"message":"{reason}"}}}}"#);
            let server = MockServer::spawn(vec![Some(http(status, &body))]);
            let error = engine_at(&server.base_url(), false, Duration::from_secs(5))
                .transcribe(&audio(), &russian())
                .unwrap_err()
                .to_string();
            assert!(error.contains(code), "{error}");
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn a_silent_provider_becomes_a_timeout_with_the_configured_seconds() {
        let server = MockServer::spawn(vec![None]);
        let error = engine_at(&server.base_url(), false, Duration::from_millis(150))
            .transcribe(&audio(), &russian())
            .unwrap_err()
            .to_string();
        assert!(error.contains("no answer"), "{error}");
        // A timeout shorter than a second is rounded up in the message, not down to zero
        assert!(error.contains("1 s"), "{error}");
    }

    #[test]
    fn without_consent_no_audio_leaves_for_another_machine() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let error = engine_at(&server.remote_base_url(), false, Duration::from_secs(2))
            .transcribe(&audio(), &russian())
            .unwrap_err()
            .to_string();
        assert!(error.contains("cloud_consent"), "{error}");
        assert_eq!(server.calls(), 0, "the audio must not reach the socket");
    }

    #[test]
    fn a_provider_on_this_machine_needs_no_consent() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        engine_at(&server.base_url(), false, Duration::from_secs(5))
            .transcribe(&audio(), &russian())
            .unwrap();
        assert_eq!(server.calls(), 1);
    }

    #[test]
    fn consent_lets_another_machine_have_the_audio() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let mut engine = engine_at(
            &format!("http://remote.invalid:{}/v1", server.addr.port()),
            true,
            Duration::from_secs(5),
        );
        // The name is not in DNS: the test gives it the address of the mock, and the client of
        // the test takes no proxy, so no machine but the mock is touched
        engine.http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("remote.invalid", server.addr)
            .build()
            .unwrap();
        engine.transcribe(&audio(), &russian()).unwrap();
        assert_eq!(server.calls(), 1);
        assert!(!audio_part(&body_of(&server.last())).is_empty());
    }

    #[test]
    fn a_cancelled_job_is_not_sent_at_all() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let mut engine = engine_at(&server.base_url(), false, Duration::from_secs(5));
        let token = Arc::new(CancelToken::new());
        token.cancel();
        engine.set_cancel(Some(Arc::clone(&token)));
        assert_eq!(
            engine.transcribe(&audio(), &russian()),
            Err(SttError::Cancelled)
        );
        assert_eq!(server.calls(), 0, "a cancelled phrase costs nothing");
    }

    #[test]
    fn empty_audio_is_refused_before_any_request() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        assert_eq!(
            engine_at(&server.base_url(), false, Duration::from_secs(5))
                .transcribe(&PcmAudio::default(), &russian()),
            Err(SttError::EmptyAudio)
        );
        assert_eq!(server.calls(), 0);
    }

    #[test]
    fn the_engine_names_itself_for_the_journal_and_keeps_the_key_out_of_the_log() {
        let engine = engine_at("http://127.0.0.1:9/v1", false, Duration::from_secs(1));
        assert_eq!(engine.id(), "remote-openai");
        assert_eq!(engine.model_name(), "remote-openai");
        let printed = format!("{engine:?}");
        assert!(!printed.contains("sk-test-0123456789abcdef"), "{printed}");
        assert!(printed.contains("sk-…cdef"), "{printed}");
    }

    #[test]
    fn unload_has_nothing_to_free() {
        // A remote provider holds no weights: unloading must not lose the address or the key
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let mut engine = engine_at(&server.base_url(), false, Duration::from_secs(5));
        engine.unload();
        engine.transcribe(&audio(), &russian()).unwrap();
        assert_eq!(server.calls(), 1);
    }

    #[test]
    fn the_timeout_of_the_settings_is_bounded() {
        let short = RemoteSttConfig {
            timeout_secs: 0,
            ..RemoteSttConfig::default()
        };
        assert_eq!(
            RemoteOpenAiEngine::from_remote(&short, "sk-x", false)
                .unwrap()
                .timeout,
            Duration::from_secs(MIN_TIMEOUT_SECS)
        );
        let long = RemoteSttConfig {
            timeout_secs: 100_000,
            ..RemoteSttConfig::default()
        };
        assert_eq!(
            RemoteOpenAiEngine::from_remote(&long, "sk-x", false)
                .unwrap()
                .timeout,
            Duration::from_secs(MAX_TIMEOUT_SECS)
        );
    }

    #[test]
    fn the_temperature_and_the_prompt_reach_the_provider() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let mut engine = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .with_temperature(0.4)
            .with_prompt("Molva, ГигаАМ");
        engine.transcribe(&audio(), &russian()).unwrap();

        let request = body_of(&server.last());
        assert!(
            find(&request, b"name=\"temperature\"\r\n\r\n0.40\r\n").is_some(),
            "{}",
            String::from_utf8_lossy(&request)
        );
        assert!(
            find(
                &request,
                "name=\"prompt\"\r\n\r\nMolva, ГигаАМ\r\n".as_bytes()
            )
            .is_some(),
            "{}",
            String::from_utf8_lossy(&request)
        );
    }

    #[test]
    fn a_default_decoder_sends_zero_and_no_prompt() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        engine_at(&server.base_url(), false, Duration::from_secs(5))
            .with_prompt("   ")
            .transcribe(&audio(), &russian())
            .unwrap();

        let request = body_of(&server.last());
        assert!(find(&request, b"name=\"temperature\"\r\n\r\n0\r\n").is_some());
        assert!(
            find(&request, b"name=\"prompt\"").is_none(),
            "a blank prompt must not occupy the window of the decoder"
        );
    }

    #[test]
    fn a_rate_limit_is_repeated_and_the_second_answer_wins() {
        let server = MockServer::spawn(vec![
            Some(http(
                "429 Too Many Requests",
                r#"{"error":{"message":"slow down"}}"#,
            )),
            Some(verbose_json()),
        ]);
        let mut engine =
            engine_at(&server.base_url(), false, Duration::from_secs(5)).with_retries(1);
        let transcript = engine.transcribe(&audio(), &russian()).unwrap();

        assert_eq!(transcript.text, "Привет, мир.");
        assert_eq!(
            server.calls(),
            2,
            "the rate limit is worth one more request"
        );
    }

    #[test]
    fn a_rejected_key_is_not_repeated() {
        let server = MockServer::spawn(vec![
            Some(http(
                "401 Unauthorized",
                r#"{"error":{"message":"bad key"}}"#,
            )),
            Some(verbose_json()),
        ]);
        let mut engine =
            engine_at(&server.base_url(), false, Duration::from_secs(5)).with_retries(3);
        let error = engine.transcribe(&audio(), &russian()).unwrap_err();

        assert!(error.to_string().contains("rejected the key"), "{error}");
        assert_eq!(
            server.calls(),
            1,
            "a key does not become right by asking again"
        );
    }

    #[test]
    fn the_retries_stop_at_the_configured_number() {
        let failed = || {
            Some(http(
                "500 Internal Server Error",
                r#"{"error":{"message":"down"}}"#,
            ))
        };
        let server = MockServer::spawn(vec![failed(), failed(), failed(), failed()]);
        let mut engine =
            engine_at(&server.base_url(), false, Duration::from_secs(5)).with_retries(2);
        let error = engine.transcribe(&audio(), &russian()).unwrap_err();

        assert!(error.to_string().contains("500"), "{error}");
        assert_eq!(server.calls(), 3, "the first request and the two repeats");
    }

    #[test]
    fn an_openrouter_request_carries_the_routing_and_the_terms() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let mut engine = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .with_prompt("Molva, ГигаАМ, транскрипция")
            .with_router(RouterPolicy {
                data_collection: "deny".into(),
                zdr: true,
                order: "groq, fireworks".into(),
                allow_fallbacks: false,
            });
        engine.transcribe(&audio(), &russian()).unwrap();

        let body = body_of(&server.last());
        let text = String::from_utf8_lossy(&body).to_string();
        let routing = find(&body, b"name=\"provider\"\r\n\r\n").expect("the provider part");
        let rest = &text[routing..];
        let json = &rest[rest.find("\r\n\r\n").unwrap() + 4..];
        let json = &json[..json.find("\r\n--").unwrap()];
        let parsed: serde_json::Value = serde_json::from_str(json).expect("the part is JSON");
        assert_eq!(parsed["data_collection"], "deny");
        assert_eq!(parsed["zdr"], true);
        assert_eq!(parsed["allow_fallbacks"], false);
        assert_eq!(parsed["order"][0], "groq");
        assert_eq!(parsed["order"][1], "fireworks");

        // Every term travels on its own, and no `prompt` part goes to a service that reads terms
        for term in ["Molva", "ГигаАМ", "транскрипция"] {
            assert!(
                text.contains(&format!("name=\"keyterms[]\"\r\n\r\n{term}\r\n")),
                "{term} is missing from the request"
            );
        }
        assert!(
            !text.contains("name=\"prompt\""),
            "OpenRouter takes the terms, not a prompt"
        );
    }

    #[test]
    fn a_service_of_its_own_gets_the_prompt_and_no_routing() {
        let server = MockServer::spawn(vec![Some(verbose_json())]);
        let mut engine = engine_at(&server.base_url(), false, Duration::from_secs(5))
            .with_prompt("Molva, ГигаАМ");
        engine.transcribe(&audio(), &russian()).unwrap();

        let text = String::from_utf8_lossy(&body_of(&server.last())).to_string();
        assert!(
            text.contains("name=\"prompt\"\r\n\r\nMolva, ГигаАМ\r\n"),
            "{text}"
        );
        assert!(!text.contains("name=\"provider\""));
        assert!(!text.contains("keyterms"));
    }

    #[test]
    fn the_routing_is_attached_only_to_an_openrouter_address() {
        let remote = RemoteSttConfig {
            base_url: "https://api.groq.com/openai/v1".into(),
            ..RemoteSttConfig::default()
        };
        assert!(
            RemoteOpenAiEngine::from_remote(&remote, "sk-x", true)
                .unwrap()
                .router
                .is_none(),
            "Groq reads neither the routing nor the terms"
        );

        for address in [
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/v1/",
            "https://eu.openrouter.ai/api/v1",
        ] {
            let remote = RemoteSttConfig {
                base_url: address.into(),
                provider_order: "groq".into(),
                ..RemoteSttConfig::default()
            };
            let router = RemoteOpenAiEngine::from_remote(&remote, "sk-x", true)
                .unwrap()
                .router
                .unwrap_or_else(|| panic!("{address} is OpenRouter and is routed"));
            assert_eq!(router.order, "groq");
        }

        // A host that only looks alike is not the service
        let remote = RemoteSttConfig {
            base_url: "https://openrouter.ai.example.com/v1".into(),
            ..RemoteSttConfig::default()
        };
        assert!(RemoteOpenAiEngine::from_remote(&remote, "sk-x", true)
            .unwrap()
            .router
            .is_none());
    }

    #[test]
    fn a_cancel_during_the_pause_leaves_without_the_next_request() {
        let failed = || {
            Some(http(
                "503 Service Unavailable",
                r#"{"error":{"message":"later"}}"#,
            ))
        };
        let server = MockServer::spawn(vec![failed(), failed(), Some(verbose_json())]);
        let token = Arc::new(CancelToken::new());
        let mut engine =
            engine_at(&server.base_url(), false, Duration::from_secs(5)).with_retries(2);
        engine.set_cancel(Some(Arc::clone(&token)));
        let cancelling = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            token.cancel();
        });

        let error = engine.transcribe(&audio(), &russian()).unwrap_err();
        cancelling.join().unwrap();

        assert!(matches!(error, SttError::Cancelled), "{error}");
        assert_eq!(
            server.calls(),
            1,
            "a cancelled job sends no further request"
        );
    }
}
