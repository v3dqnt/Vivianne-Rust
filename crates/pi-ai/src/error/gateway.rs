//! Port of `src/error/gateway.ts`.

use std::sync::OnceLock;

use regex::Regex;

use super::flags::{is_usage_limit, ErrorLike, ErrorShape, TextError};

/// A gateway-facing classification of an arbitrary upstream/internal error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayErrorClassification {
    pub status: u32,
    pub kind: String,
    pub message: String,
}

fn class(status: u32, kind: &str, message: &str) -> GatewayErrorClassification {
    GatewayErrorClassification { status, kind: kind.to_owned(), message: message.to_owned() }
}

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            C.get_or_init(|| Regex::new($pat).expect("static pattern"))
        }
    };
}

lazy_re!(aborted_text, r"(?i)\baborted\b|\babort signal\b");
// Match rate-limit phrasings before auth wording: some providers describe throttling as
// "unauthorized due to rate limit". Keep boundaries so this does not collide with
// `GenerateContentRequest`, `accelerate`, `iterate`, `deprecated`, etc.
lazy_re!(
    rate_limit_text,
    r"(?i)\brate[- _]?limit(?:s|ed|ing)?\b|\bquota(?:_exceeded| exceeded)?\b|\btoo[- _]many[- _]requests\b"
);
lazy_re!(auth_text, r"(?i)\b(?:unauthorized|forbidden)\b");
lazy_re!(invalid_request_text, r"(?i)\b(?:unsupported|invalid_request|invalid request|bad request|malformed)\b");
// `Google API error (400)`, `OpenAI API error (429): ...`, `(503)`; `HTTP 429: too many requests`;
// `status: 503`, `status_code=429`, `status=400`
lazy_re!(
    embedded_status,
    r"(?i)(?:\bHTTP\b|\bAPI error\b|\bstatus(?:[- _]?code)?\b)\s*[:=]?\s*\(?\s*(\d{3})\b|\((\d{3})\)"
);

/// What the gateway classifier reads off an error: its message text, an optional numeric `status`
/// property, and whether it is an `AbortError`.
pub trait GatewayError: ErrorLike {
    /// `err.name === "AbortError"`.
    fn is_abort_error(&self) -> bool {
        false
    }
}

/// Classify an upstream / gateway-internal error into a status code and a format-neutral type. The
/// order is intentional:
///
///  1. Honour an explicit numeric `status` property on the thrown error.
///  2. Parse a status code embedded in the message string. Provider errors virtually always carry
///     one (`Google API error (400): ...`, `HTTP 429`, `status=503`) and the embedded value is
///     authoritative.
///  3. Fall through to **word-boundaried** substring heuristics. The old `lower.includes("rate")`
///     test famously matched `GenerateContentRequest`, surfacing every Google 400 as a 429
///     `rate_limit_error`. The patterns here all require boundaries so they don't collide with
///     provider field names.
pub fn classify_gateway_error(err: &dyn GatewayError) -> GatewayErrorClassification {
    let message = err.message().unwrap_or("").to_owned();
    let message = message.as_str();

    // 1. Custom pi-ai errors may attach a numeric `status` property.
    if let Some(status_prop) = err.own_status() {
        return bucket_status(status_prop, message);
    }

    if err.shape() == ErrorShape::Error && err.is_abort_error() {
        return class(499, "request_aborted", message);
    }

    // 2. Status code embedded in the message. Requires a contextual keyword (`HTTP`, `API error`,
    // `status`, ...) or a leading `(NNN)` token so we don't trip on incidental three-digit numbers
    // ("took 200ms").
    if let Some(embedded) = extract_embedded_status(message) {
        return bucket_status(embedded, message);
    }

    // 3. Word-boundaried substring heuristics.
    if aborted_text().is_match(message) {
        return class(499, "request_aborted", message);
    }
    if rate_limit_text().is_match(message)
        // Usage-limit phrasings emit no embedded status. Codex friendly text reads "You have hit
        // your ChatGPT usage limit ... Try again in ~158 min."; the central usage-limit classifier
        // already encodes every known provider variant, so reuse it instead of forking the regex.
        // Without this branch the classifier falls through to the default 502/upstream_error, which
        // is what callers saw when their account hit its cap.
        || is_usage_limit(&TextError(message), None)
    {
        return class(429, "rate_limit_error", message);
    }
    if auth_text().is_match(message) {
        return class(401, "authentication_error", message);
    }
    if invalid_request_text().is_match(message) {
        return class(400, "invalid_request_error", message);
    }
    class(502, "upstream_error", message)
}

fn bucket_status(status: u32, message: &str) -> GatewayErrorClassification {
    if status == 401 || status == 403 {
        return class(status, "authentication_error", message);
    }
    if status == 429 {
        return class(status, "rate_limit_error", message);
    }
    if (400..500).contains(&status) {
        return class(status, "invalid_request_error", message);
    }
    if status >= 500 {
        return class(status, "upstream_error", message);
    }
    class(502, "upstream_error", message)
}

/// Pull a status code from common error-message shapes. Returns `None` when no contextual keyword
/// is present, so we never guess at incidental numbers.
fn extract_embedded_status(message: &str) -> Option<u32> {
    let m = embedded_status().captures(message)?;
    let raw = m.get(1).or_else(|| m.get(2))?.as_str();
    let code = raw.parse::<u32>().ok()?;
    if (100..600).contains(&code) {
        Some(code)
    } else {
        None
    }
}

/// A plain error or string for the gateway classifier.
#[derive(Debug, Clone)]
pub struct GatewayText {
    pub message: String,
    pub status: Option<u32>,
    pub is_abort: bool,
    pub is_string: bool,
}

impl GatewayText {
    pub fn error(message: impl Into<String>) -> Self {
        Self { message: message.into(), status: None, is_abort: false, is_string: false }
    }
    /// `String(err)` for a non-Error value.
    pub fn string(message: impl Into<String>) -> Self {
        Self { message: message.into(), status: None, is_abort: false, is_string: true }
    }
}

impl ErrorLike for GatewayText {
    fn message(&self) -> Option<&str> {
        Some(&self.message)
    }
    fn own_status(&self) -> Option<u32> {
        self.status
    }
    fn shape(&self) -> ErrorShape {
        if self.is_string { ErrorShape::String } else { ErrorShape::Error }
    }
}

impl GatewayError for GatewayText {
    fn is_abort_error(&self) -> bool {
        self.is_abort
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> (u32, String) {
        let c = classify_gateway_error(&GatewayText::error(text));
        (c.status, c.kind)
    }

    #[test]
    fn embedded_status_wins_and_buckets() {
        assert_eq!(run("Google API error (400): GenerateContentRequest invalid"), (400, "invalid_request_error".into()));
        assert_eq!(run("HTTP 429: slow down"), (429, "rate_limit_error".into()));
        assert_eq!(run("status=503 upstream"), (503, "upstream_error".into()));
        assert_eq!(run("OpenAI API error (403): no"), (403, "authentication_error".into()));
        assert_eq!(run("took 200ms"), (502, "upstream_error".into()));
    }

    #[test]
    fn text_heuristics_do_not_collide_with_field_names() {
        assert_eq!(run("GenerateContentRequest.contents is required, bad request"), (400, "invalid_request_error".into()));
        assert_eq!(run("rate limit hit"), (429, "rate_limit_error".into()));
        assert_eq!(run("You have hit your ChatGPT usage limit"), (429, "rate_limit_error".into()));
        assert_eq!(run("Unauthorized"), (401, "authentication_error".into()));
        assert_eq!(run("request was aborted by user"), (499, "request_aborted".into()));
    }

    #[test]
    fn explicit_status_and_abort_name() {
        let mut e = GatewayText::error("whatever");
        e.status = Some(418);
        assert_eq!(classify_gateway_error(&e).status, 418);
        e.status = Some(700);
        assert_eq!(classify_gateway_error(&e).status, 700);
        e.status = Some(99);
        assert_eq!(classify_gateway_error(&e).status, 502);
        let mut a = GatewayText::error("x");
        a.is_abort = true;
        assert_eq!(classify_gateway_error(&a).kind, "request_aborted");
    }
}
