//! Port of `src/error/classes.ts`.
//!
//! Not ported yet: `OpenAIHttpError` (needs `CapturedHttpErrorResponse` from `utils/http-inspector`)
//! and `AnthropicConnectionError` (needs `network_failure_message` from `error/network-cause`).
//! They land with those modules in the next phase.

use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use tokio_util::sync::CancellationToken;

use super::abort::AbortError;

/// Prefix on errors raised when an Anthropic SSE stream envelope is malformed.
pub const STREAM_ENVELOPE_ERROR_PREFIX: &str = "Anthropic stream envelope error:";

/// Structured HTTP errors thrown by provider clients.
#[derive(Debug, Default)]
pub struct ProviderHttpErrorOptions {
    /// Response headers; enables `retry-after`/rate-limit extraction downstream.
    pub headers: Option<HeaderMap>,
    /// Machine-readable error code from the response body (`error.code` / `error.type`).
    pub code: Option<String>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

/// Non-2xx HTTP response from a provider.
#[derive(Debug)]
pub struct ProviderHttpError {
    /// The TS `this.name`; subclasses set their own.
    pub name: &'static str,
    pub message: String,
    pub status: u32,
    pub headers: Option<HeaderMap>,
    pub code: Option<String>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// The classification set by `attach(error, id)`.
    pub error_id: Option<u32>,
}

impl ProviderHttpError {
    pub fn new(message: impl Into<String>, status: u32, options: ProviderHttpErrorOptions) -> Self {
        Self {
            name: "ProviderHttpError",
            message: message.into(),
            status,
            headers: options.headers,
            code: options.code,
            cause: options.cause,
            error_id: None,
        }
    }

    /// `attach(error, id)`.
    #[must_use]
    pub fn attach(mut self, id: u32) -> Self {
        self.error_id = Some(id);
        self
    }
}

impl super::flags::HasErrorId for ProviderHttpError {
    fn error_id(&self) -> Option<u32> {
        self.error_id
    }
}

impl std::fmt::Display for ProviderHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProviderHttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}

/// Pull a human-readable message and machine code out of an OpenAI-style error envelope
/// (`{ error: { message, code, type } }`), tolerating the flat shapes compat hosts return
/// (`{ error: "..." }`, `{ message: "..." }`) and falling back to the raw body text.
///
/// This is `OpenAIHttpError.parseEnvelope`; the class itself lands with `http-inspector`.
pub fn parse_openai_error_envelope(
    body_json: Option<&serde_json::Value>,
    body_text: Option<&str>,
) -> (Option<String>, Option<String>) {
    if let Some(serde_json::Value::Object(envelope)) = body_json {
        match envelope.get("error") {
            Some(serde_json::Value::Object(error)) => {
                let message = error.get("message").and_then(|v| v.as_str());
                let detail = match message {
                    Some(m) if !m.is_empty() => Some(m.to_owned()),
                    _ => body_text.map(str::to_owned),
                };
                let code = error
                    .get("code")
                    .and_then(|v| v.as_str())
                    .or_else(|| error.get("type").and_then(|v| v.as_str()))
                    .map(str::to_owned);
                return (detail, code);
            }
            Some(serde_json::Value::String(s)) if !s.is_empty() => return (Some(s.clone()), None),
            _ => {}
        }
        if let Some(serde_json::Value::String(m)) = envelope.get("message") {
            if !m.is_empty() {
                return (Some(m.clone()), None);
            }
        }
    }
    (body_text.map(str::to_owned), None)
}

const DEFAULT_ANTHROPIC_ERROR_BODY_READ_TIMEOUT_MS: u64 = 5_000;
const MAX_ANTHROPIC_ERROR_BODY_BYTES: usize = 64 * 1024;
const ANTHROPIC_ERROR_BODY_TRUNCATION_MARKER: &str = "\n[Response body truncated after 64 KiB]";

static ANTHROPIC_ERROR_BODY_READ_TIMEOUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(DEFAULT_ANTHROPIC_ERROR_BODY_READ_TIMEOUT_MS);

/// Test-only control for the otherwise bounded Anthropic error-body drain.
/// `None` restores the default.
pub fn anthropic_api_error_set_body_read_timeout_ms_for_testing(timeout_ms: Option<f64>) -> Result<(), String> {
    if let Some(t) = timeout_ms {
        if !t.is_finite() || t < 0.0 {
            return Err("Anthropic error-body timeout must be a non-negative finite number.".to_owned());
        }
    }
    ANTHROPIC_ERROR_BODY_READ_TIMEOUT_MS.store(
        timeout_ms.map_or(DEFAULT_ANTHROPIC_ERROR_BODY_READ_TIMEOUT_MS, |t| t as u64),
        std::sync::atomic::Ordering::SeqCst,
    );
    Ok(())
}

/// Non-2xx response from the Anthropic API.
#[derive(Debug)]
pub struct AnthropicApiError {
    pub base: ProviderHttpError,
    pub request_id: Option<String>,
}

impl AnthropicApiError {
    pub fn new(status: u32, message: impl Into<String>, headers: HeaderMap, code: Option<String>, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        let request_id = headers.get("request-id").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let mut base = ProviderHttpError::new(message, status, ProviderHttpErrorOptions { headers: Some(headers), code, cause });
        base.name = "AnthropicApiError";
        Self { base, request_id }
    }

    /// Read a bounded slice of an error response's body and build the error from it.
    pub async fn from_response(
        mut response: reqwest::Response,
        signal: Option<&CancellationToken>,
    ) -> Result<AnthropicApiError, AbortError> {
        let status = response.status().as_u16() as u32;
        let headers = response.headers().clone();
        let timeout_ms = ANTHROPIC_ERROR_BODY_READ_TIMEOUT_MS.load(std::sync::atomic::Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        if signal.map_or(false, |s| s.is_cancelled()) {
            return Err(AbortError::new(Some("Request was aborted."), None));
        }

        let mut captured: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut clean_eof = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let next = response.chunk();
            let chunk = tokio::select! {
                _ = async { match signal { Some(s) => s.cancelled().await, None => std::future::pending().await } } => {
                    return Err(AbortError::new(Some("Request was aborted."), None));
                }
                r = tokio::time::timeout(remaining, next) => match r {
                    Err(_) => break,        // timed out
                    Ok(Err(_)) => break,    // read error
                    Ok(Ok(c)) => c,
                },
            };
            match chunk {
                None => {
                    clean_eof = true;
                    break;
                }
                Some(bytes) => {
                    let room = MAX_ANTHROPIC_ERROR_BODY_BYTES - captured.len();
                    let take = room.min(bytes.len());
                    captured.extend_from_slice(&bytes[..take]);
                    if take < bytes.len() {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        if signal.map_or(false, |s| s.is_cancelled()) {
            return Err(AbortError::new(Some("Request was aborted."), None));
        }
        let mut body = String::from_utf8_lossy(&captured).into_owned();
        let _ = clean_eof;
        if truncated {
            body.push_str(ANTHROPIC_ERROR_BODY_TRUNCATION_MARKER);
        }
        let trimmed = body.trim();
        let detail = if trimmed.is_empty() { "status code (no body)".to_owned() } else { trimmed.to_owned() };
        let code = anthropic_error_code(&detail);
        Ok(AnthropicApiError::new(status, format!("{status} {detail}"), headers, code, None))
    }
}

/// `error.details.error_code`, else `error.type`, from an Anthropic error body.
fn anthropic_error_code(detail: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(detail).ok()?;
    let err = parsed.as_object()?.get("error")?.as_object()?;
    if let Some(code) = err
        .get("details")
        .and_then(|d| d.as_object())
        .and_then(|d| d.get("error_code"))
        .and_then(|c| c.as_str())
    {
        return Some(code.to_owned());
    }
    err.get("type").and_then(|t| t.as_str()).map(str::to_owned)
}

impl std::ops::Deref for AnthropicApiError {
    type Target = ProviderHttpError;
    fn deref(&self) -> &ProviderHttpError {
        &self.base
    }
}
impl std::fmt::Display for AnthropicApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.base, f)
    }
}
impl std::error::Error for AnthropicApiError {}

/// No response headers arrived within the configured request timeout.
#[derive(Debug, Clone, Default)]
pub struct AnthropicConnectionTimeoutError;

impl std::fmt::Display for AnthropicConnectionTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Request timed out.")
    }
}
impl std::error::Error for AnthropicConnectionTimeoutError {}

/// A malformed or truncated Anthropic SSE stream envelope: events arriving out of order (before
/// `message_start`), a retryable in-band `error` frame that ends the message before its terminal
/// frame, or otherwise a stream that never reached that frame. A deterministic provider rejection
/// keeps its completed-turn error instead. The message is prefixed with
/// [`STREAM_ENVELOPE_ERROR_PREFIX`] so the shared envelope predicates classify it.
#[derive(Debug, Clone)]
pub struct AnthropicStreamEnvelopeError {
    pub message: String,
}

impl AnthropicStreamEnvelopeError {
    pub fn new(detail: &str) -> Self {
        Self { message: format!("{STREAM_ENVELOPE_ERROR_PREFIX} {detail}") }
    }
}
impl std::fmt::Display for AnthropicStreamEnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for AnthropicStreamEnvelopeError {}

macro_rules! http_subclass {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name(pub ProviderHttpError);

        impl $name {
            pub const NAME: &'static str = stringify!($name);
            pub fn new(message: impl Into<String>, status: u32, options: ProviderHttpErrorOptions) -> Self {
                let mut inner = ProviderHttpError::new(message, status, options);
                inner.name = Self::NAME;
                Self(inner)
            }
        }
        impl std::ops::Deref for $name {
            type Target = ProviderHttpError;
            fn deref(&self) -> &ProviderHttpError {
                &self.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl std::error::Error for $name {}
    };
}

http_subclass!(
    /// Non-2xx response (or in-stream exception event) from the Bedrock runtime API.
    BedrockApiError
);
http_subclass!(
    /// Non-2xx response (or in-stream error chunk) from the Cloud Code Assist API.
    GeminiCliApiError
);
http_subclass!(
    /// Non-2xx response (or in-stream error chunk) from the Google Generative Language / Vertex API.
    GoogleApiError
);
http_subclass!(
    /// Non-2xx response from the Ollama `/api/chat` endpoint.
    OllamaApiError
);

/// Auth gateway HTTP failure.
#[derive(Debug)]
pub struct AuthGatewayError(pub ProviderHttpError);

impl AuthGatewayError {
    pub fn new(message: impl Into<String>, status: u32, headers: Option<HeaderMap>, code: Option<String>) -> Self {
        let mut inner = ProviderHttpError::new(message, status, ProviderHttpErrorOptions { headers, code, cause: None });
        inner.name = "AuthGatewayError";
        Self(inner)
    }
}
impl std::ops::Deref for AuthGatewayError {
    type Target = ProviderHttpError;
    fn deref(&self) -> &ProviderHttpError {
        &self.0
    }
}
impl std::fmt::Display for AuthGatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
impl std::error::Error for AuthGatewayError {}

#[derive(Debug, Clone)]
pub struct CodexWebSocketTransportError {
    pub message: String,
}

impl CodexWebSocketTransportError {
    pub fn new(detail: &str) -> Self {
        Self { message: format!("Codex websocket transport failure: {detail}") }
    }
}
impl std::fmt::Display for CodexWebSocketTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CodexWebSocketTransportError {}

#[derive(Debug, Clone)]
pub struct CodexWhitespaceToolCallLoopError {
    pub message: String,
}

impl CodexWhitespaceToolCallLoopError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}
impl std::fmt::Display for CodexWhitespaceToolCallLoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CodexWhitespaceToolCallLoopError {}

#[derive(Debug)]
pub struct CodexProviderStreamError {
    pub message: String,
    pub retryable: bool,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl CodexProviderStreamError {
    /// `retryable` defaults to true unless explicitly `Some(false)`.
    pub fn new(message: impl Into<String>, retryable: Option<bool>, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self { message: message.into(), retryable: retryable != Some(false), cause }
    }
}
impl std::fmt::Display for CodexProviderStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CodexProviderStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug, Default)]
pub struct AuthBrokerErrorOptions {
    pub status: Option<u32>,
    pub body: Option<String>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

#[derive(Debug)]
pub struct AuthBrokerError {
    pub name: &'static str,
    pub message: String,
    pub status: Option<u32>,
    pub body: Option<String>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl AuthBrokerError {
    pub fn new(message: impl Into<String>, opts: AuthBrokerErrorOptions) -> Self {
        Self { name: "AuthBrokerError", message: message.into(), status: opts.status, body: opts.body, cause: opts.cause }
    }
}
impl std::fmt::Display for AuthBrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for AuthBrokerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug)]
pub struct AuthBrokerStreamUnsupportedError(pub AuthBrokerError);

impl AuthBrokerStreamUnsupportedError {
    pub fn new(message: Option<&str>) -> Self {
        let mut inner = AuthBrokerError::new(
            message.unwrap_or("Auth broker does not support /v1/snapshot/stream"),
            AuthBrokerErrorOptions { status: Some(404), ..Default::default() },
        );
        inner.name = "AuthBrokerStreamUnsupportedError";
        Self(inner)
    }
}
impl std::ops::Deref for AuthBrokerStreamUnsupportedError {
    type Target = AuthBrokerError;
    fn deref(&self) -> &AuthBrokerError {
        &self.0
    }
}
impl std::fmt::Display for AuthBrokerStreamUnsupportedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
impl std::error::Error for AuthBrokerStreamUnsupportedError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_parse_handles_nested_flat_and_text() {
        let nested = serde_json::json!({"error": {"message": "bad", "code": "c1", "type": "t"}});
        assert_eq!(parse_openai_error_envelope(Some(&nested), Some("raw")), (Some("bad".into()), Some("c1".into())));
        let by_type = serde_json::json!({"error": {"type": "t"}});
        assert_eq!(parse_openai_error_envelope(Some(&by_type), Some("raw")), (Some("raw".into()), Some("t".into())));
        let flat = serde_json::json!({"error": "boom"});
        assert_eq!(parse_openai_error_envelope(Some(&flat), None), (Some("boom".into()), None));
        let msg = serde_json::json!({"message": "m"});
        assert_eq!(parse_openai_error_envelope(Some(&msg), None), (Some("m".into()), None));
        assert_eq!(parse_openai_error_envelope(None, Some("t")), (Some("t".into()), None));
    }

    #[test]
    fn anthropic_code_prefers_details_error_code() {
        let a = r#"{"error":{"type":"overloaded_error","details":{"error_code":"rate_limit"}}}"#;
        assert_eq!(anthropic_error_code(a).as_deref(), Some("rate_limit"));
        assert_eq!(anthropic_error_code(r#"{"error":{"type":"x"}}"#).as_deref(), Some("x"));
        assert_eq!(anthropic_error_code("not json"), None);
    }

    #[test]
    fn subclass_names_and_envelope_prefix() {
        assert_eq!(BedrockApiError::new("m", 500, Default::default()).name, "BedrockApiError");
        assert_eq!(
            AnthropicStreamEnvelopeError::new("x").to_string(),
            "Anthropic stream envelope error: x"
        );
        assert_eq!(AuthBrokerStreamUnsupportedError::new(None).status, Some(404));
        assert!(CodexProviderStreamError::new("m", None, None).retryable);
        assert!(!CodexProviderStreamError::new("m", Some(false), None).retryable);
    }

    #[test]
    fn anthropic_api_error_reads_request_id() {
        let mut h = HeaderMap::new();
        h.insert("request-id", "req_1".parse().unwrap());
        let e = AnthropicApiError::new(529, "529 x", h, None, None);
        assert_eq!(e.request_id.as_deref(), Some("req_1"));
        assert_eq!(e.status, 529);
    }
}
