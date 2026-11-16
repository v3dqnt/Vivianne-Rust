//! Port of `src/error/body-error.ts`.
//!
//! In-band provider failures: upstream 429/5xx payloads that arrive inside an HTTP 200 response
//! body, or mid-stream after the SSE headers were already sent.
//!
//! Providers that front a retry/queue layer (Azure OpenAI, LiteLLM-style aggregators,
//! Bedrock-compatible shims) answer a throttled request with `200 OK` + `text/event-stream` and put
//! the real status in the payload: `data: {"error":{"type":"rate_limit_error"}}`,
//! `data: {"code":429}`, or a bare non-JSON frame such as `data: 429 Too Many Requests` / an nginx
//! throttle page. Those bodies used to be either dropped silently (the stream then looked like a
//! successful empty completion) or surfaced as an unclassified [`ProviderResponseError`] whose
//! `errorId` stayed 0, so `retriable` answered "terminal" and `retry.fallbackChains` never advanced,
//! pinning the session on a provider that was merely busy.
//!
//! Both probes hand the classifier the structured signal it already trusts for an out-of-band
//! failure: a [`ProviderHttpError`] carrying a status the upstream *reported*, so the two routes
//! reach one code path. The invariants that make that safe, and that callers rely on:
//!
//!  - **No invented HTTP metadata.** A status is taken only from an error `status`/`code` *field*
//!    (numeric, or a 3-digit string as compat hosts send) or from `RETRYABLE_STATUS_BY_CODE`, and
//!    only for `429`/`5xx`. A status mentioned inside error prose is never promoted to a status:
//!    401/403 wording stays text and cannot route the failure into the auth-retry lane.
//!  - **No credential rotation on an unreadable body.** A `429` whose body is empty, `{}`, or
//!    framing-only is opaque, and an opaque 429 is the conservative rotate-to-a-sibling-credential
//!    signal. That verdict belongs to a body the server actually sent, not to a payload we
//!    synthesised, so `format_in_band_message` substitutes `IN_BAND_DETAIL_PLACEHOLDER` and the
//!    composed message always stays informative.
//!  - **Unknown envelopes fall through.** Anything without a retryable status field, a retryable
//!    code, or unambiguous throttle wording returns `None` and keeps its pre-existing handling and
//!    message.
//!
//! Where no numeric status exists, the returned error keeps the upstream wording and code visible
//! in its message instead of asserting HTTP metadata the provider never sent, and carries
//! `Flag::Transient` directly: throttle spellings like `Throttled` / `Please retry` are what this
//! probe recognises but the transport text pattern is not required to match, so the retry decision
//! is stated rather than hoped for.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::classes::{ProviderHttpError, ProviderHttpErrorOptions};
use super::flags::{create, Flag};
use super::provider::{ProviderResponseError, ProviderResponseErrorKind, ProviderResponseErrorOptions};
use super::rate_limit::is_opaque_status_body;

/// Cap on synthesized message length, mirroring the transport-level `MAX_DETAIL_CHARS`.
const MAX_IN_BAND_DETAIL_CHARS: usize = 4096;

/// Filler used when an in-band failure frame carries no readable detail. It has to be informative
/// prose on purpose: an opaque message on a `429` is read as "the server gave us nothing" and
/// rotates a credential, and that judgement must never be triggered by wording of ours.
const IN_BAND_DETAIL_PLACEHOLDER: &str = "Provider returned an in-band provider error";

/// Machine error codes that mean "shed this request and back off", mapped to the HTTP status the
/// upstream would have used had it not wrapped the failure in a 200. Keys are compared after
/// lower-casing, splitting camel/Pascal word boundaries, and collapsing `_`/`-`/`.`/spaces, so
/// `Throttling.AllocationQuota` and `ThrottlingAllocationQuota` both resolve. The list is
/// deliberately limited to throttle/overload spellings so an unlisted code keeps its pre-existing
/// classification: request-validation failures (`invalid_request_error`) and account caps
/// (`insufficient_quota`, `usage_limit_reached`) are never reinterpreted as retries.
const RETRYABLE_STATUS_BY_CODE: [(&str, u32); 23] = [
    ("rate_limit_error", 429),
    ("rate_limit_exceeded", 429),
    ("rate_limit", 429),
    ("rate_limit_reached", 429),
    ("rate_limited", 429),
    ("ratelimit", 429),
    ("too_many_requests", 429),
    ("request_throttled", 429),
    ("throttled", 429),
    ("throttling", 429),
    ("throttling_error", 429),
    ("throttling_exception", 429),
    ("throttling_allocation_quota", 429),
    ("request_limit_exceeded", 429),
    ("retry_later", 429),
    ("overloaded_error", 503),
    ("server_overloaded", 503),
    ("model_overloaded", 503),
    ("overloaded", 503),
    ("service_unavailable", 503),
    ("server_busy", 503),
    ("high_demand", 503),
    ("capacity_exceeded", 503),
];

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            C.get_or_init(|| Regex::new($pat).expect("static pattern"))
        }
    };
}

// Wording that identifies a throttle/overload in an error code or body. Kept as an explicit list
// rather than reusing the classifier's pattern, so this probe stays independent of the text rules
// it feeds.
lazy_re!(
    in_band_retryable_text,
    r"(?i)\brate.?limit|too many requests|too\s+many\s+concurren|service.{0,20}unavailable|temporarily\s+unavailable|server.?error|internal.?error|overloaded|capacity|throttl|retry\s+(?:your\s+)?request|please\s+retry"
);
// A status in the position a proxy error page puts it: the very first token, optionally after an
// `HTTP/1.1 ` prefix. Delimited by a word boundary so identifiers (`chatcmpl-500321`, `gpt-500x`,
// `req500502`) cannot fabricate a status. Prose that merely *mentions* a number
// (`Too many requests (401 from ...)`) is deliberately not read as status metadata.
lazy_re!(leading_status, r"(?i)^\s*(?:HTTP[/.]\d(?:\.\d)?\s+)?([45]\d{2})(?:\b|$)");
// Codes that mean a persistent account/billing cap or a bad request; never shed-and-retry.
lazy_re!(
    non_retryable_code,
    r"(?i)insufficient.?quota|usage.?limit|quota.?(?:exceeded|reached|insufficient)|invalid_request|content_filter|context_length|context_window|billing|balance"
);
lazy_re!(camel_boundary, r"([a-z0-9])([A-Z])");
lazy_re!(code_separators, r"[-.\s]+");
lazy_re!(code_underscores, r"_+");
lazy_re!(html_tag, r"<[^>]*>");
lazy_re!(control_chars, r"[\x00-\x1f\x7f]+");
lazy_re!(whitespace_run, r"\s+");
lazy_re!(three_digits, r"^\d{3}$");
lazy_re!(all_digits, r"^\d+$");

/// Flags this module asserts for a body it has itself recognised as shed-and-retry.
fn in_band_flags() -> u32 {
    create(&[Flag::Transient])
}

fn normalize_code_token(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i.to_string())
            } else {
                n.as_f64().filter(|f| f.is_finite()).map(|f| f.to_string())
            }
        }
        Value::String(s) => {
            let spaced = camel_boundary().replace_all(s.trim(), "${1}_${2}");
            let lowered = spaced.to_lowercase();
            let collapsed = code_separators().replace_all(&lowered, "_");
            let collapsed = code_underscores().replace_all(&collapsed, "_").into_owned();
            if collapsed.is_empty() { None } else { Some(collapsed) }
        }
        _ => None,
    }
}

fn read_in_band_detail(value: Option<&Value>) -> Option<String> {
    let Value::String(value) = value? else { return None };
    read_in_band_detail_str(value)
}

fn read_in_band_detail_str(value: &str) -> Option<String> {
    // SSE joins multi-line `data:` values with `\n`, and proxy failures arrive as HTML: flatten to
    // one line of visible text so a synthesized message cannot smuggle markup or framing into the
    // classifier or the terminal.
    let no_tags = html_tag().replace_all(value, " ");
    let no_ctrl = control_chars().replace_all(&no_tags, " ");
    let squeezed = whitespace_run().replace_all(&no_ctrl, " ");
    let flattened = squeezed.trim();
    if flattened.is_empty() {
        return None;
    }
    Some(flattened.chars().take(MAX_IN_BAND_DETAIL_CHARS).collect())
}

/// A numeric HTTP status field, accepting the `"429"` string form compat hosts emit.
fn read_status_field(value: Option<&Value>) -> Option<u32> {
    match value? {
        Value::Number(n) => n.as_u64().filter(|v| (100..=599).contains(v)).map(|v| v as u32),
        Value::String(s) if three_digits().is_match(s.trim()) => {
            s.trim().parse::<u32>().ok().filter(|v| (100..=599).contains(v))
        }
        _ => None,
    }
}

/// Only `429` and genuine server faults are shed-and-retry; 4xx of any other kind is not.
fn is_retryable_status(status: Option<u32>) -> bool {
    matches!(status, Some(s) if s == 429 || (500..=599).contains(&s))
}

fn read_leading_status(text: Option<&str>) -> Option<u32> {
    leading_status().captures(text?).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok())
}

#[derive(Debug, Default)]
struct InBandSignal {
    /// Numeric HTTP status the upstream reported, or implied by its error code.
    status: Option<u32>,
    /// Machine code from the body (`error.code` preferred over `error.type`).
    code: Option<String>,
    /// Human-readable detail from the body.
    detail: Option<String>,
}

/// `a ?? b` over optional JSON values: `null` and missing both fall through.
fn coalesce<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    match a {
        Some(Value::Null) | None => b,
        some => some,
    }
}

/// Pull the failure signal out of an OpenAI-wire frame. Accepts the nested
/// `{ error: { code, type, status, message } }` shape, the `response.error` position of
/// Responses-API terminal events, the flat `{ code, status, message }` bodies compat hosts emit,
/// and string envelopes (`{ error: "..." }`).
///
/// `None` means "not an in-band failure worth retrying". The probe is intentionally narrow: a bare
/// `type` is present on every Responses event and so never counts as a signal by itself, statuses
/// come only from fields (never from prose), and a frame with neither a retryable status nor
/// throttle wording is left to the caller's existing handling.
fn read_in_band_signal(frame: &Value) -> Option<InBandSignal> {
    let root = frame.as_object()?;
    let nested: Option<&Value> =
        coalesce(root.get("error"), root.get("response").and_then(|r| r.as_object()).and_then(|r| r.get("error")));
    // Azure-compatible gates double-wrap (`{ error: { error: { code, message } } }`). Walk at most
    // two levels so a deep payload cannot extend the parse.
    let mut error: Option<&serde_json::Map<String, Value>> = None;
    if let Some(Value::Object(n)) = nested {
        let mut cur = n;
        for _ in 0..2 {
            match cur.get("error") {
                Some(Value::Object(inner)) => cur = inner,
                _ => break,
            }
        }
        error = Some(cur);
    }
    let get_err = |k: &str| error.and_then(|e| e.get(k));
    let code = normalize_code_token(coalesce(get_err("code"), root.get("code")))
        .or_else(|| normalize_code_token(coalesce(get_err("type"), root.get("type"))));
    if code.as_deref().map_or(false, |c| non_retryable_code().is_match(c)) {
        return None;
    }
    // A flat retryable code/type is itself an in-band failure: Responses-API `error` events expose
    // `{ type: "rate_limit_error" }` with no `error` member, and their handler passes the inner
    // error object (not the whole event).
    let retryable_code = code.as_deref().map_or(false, |c| in_band_retryable_text().is_match(c));
    // Only an explicit error member, a top-level status/code/message field, or a standalone
    // throttle type can qualify a frame as a failure; ordinary chunks carry none of these. A bare
    // `type` is present on every Responses event, so it only counts when it is itself retryable
    // wording.
    if !retryable_code
        && nested.is_none()
        && !root.contains_key("status")
        && !root.contains_key("code")
        && !root.contains_key("message")
    {
        return None;
    }
    let detail = read_in_band_detail(get_err("message"))
        .or_else(|| read_in_band_detail(root.get("message")))
        .or_else(|| match nested {
            Some(Value::String(s)) => read_in_band_detail_str(s),
            _ => None,
        });
    // Reported statuses come from fields only: the error member's `status`, its numeric `code`,
    // then the same on the root (the flat `{ code: 429 }` / `{ status: 429 }` bodies compat hosts
    // emit). Anything outside 429/5xx is ignored outright: an in-band `400`/`401`/`403` field must
    // not become a synthetic HTTP contract, or a body that merely names an auth problem would
    // route into the credential lane.
    let reported: Vec<Option<&Value>> = match error {
        None => vec![root.get("status"), root.get("code")],
        Some(e) => vec![e.get("status"), e.get("code"), root.get("status"), root.get("code")],
    };
    let status = reported
        .into_iter()
        .map(read_status_field)
        .find(|s| is_retryable_status(*s))
        .flatten()
        .or_else(|| {
            code.as_deref()
                .and_then(|c| RETRYABLE_STATUS_BY_CODE.iter().find(|(k, _)| *k == c))
                .map(|(_, status)| *status)
        });
    if status.is_some() {
        return Some(InBandSignal { status, code, detail });
    }
    // No reported status: classify only when the upstream *message* is itself unambiguous throttle
    // wording. A generic code alone must not qualify: Azure uses `server_error` for terminal
    // backend failures whose `"<code>: <message>"` envelope the caller already reports (and whose
    // text the shared transient rule already matches), so intercepting it would change an
    // established error format without adding retry information.
    let detail_text = detail.as_deref()?;
    if !in_band_retryable_text().is_match(detail_text) {
        return None;
    }
    Some(InBandSignal { status: None, code, detail })
}

/// Compose the message for a status-bearing in-band failure. The numeric status leads (matching
/// `captureOpenAIHttpError`'s `"<status> <detail>"` phrasing) unless the detail already carries it
/// as its leading token; the machine code is appended only when it adds information the text
/// classifier or a human reader can use. A detail that leaves the whole line opaque is replaced by
/// the placeholder, so a body we generated can never be read as "the server said nothing".
fn format_in_band_message(status: u32, detail: Option<&str>, code: Option<&str>) -> String {
    let body = detail.unwrap_or(IN_BAND_DETAIL_PLACEHOLDER);
    let suffix = match code {
        Some(c) if !all_digits().is_match(c) && !body.to_lowercase().contains(&c.to_lowercase()) => format!(" ({c})"),
        _ => String::new(),
    };
    let mut message = if read_leading_status(Some(body)) == Some(status) {
        body.to_owned()
    } else {
        format!("{status} {body}")
    };
    if is_opaque_status_body(&message) {
        message = format!("{status} {IN_BAND_DETAIL_PLACEHOLDER}");
    }
    format!("{message}{suffix}")
}

/// The classified error an in-band failure frame becomes.
#[derive(Debug)]
pub enum InBandError {
    Http(ProviderHttpError),
    Response(ProviderResponseError),
}

impl InBandError {
    pub fn message(&self) -> &str {
        match self {
            Self::Http(e) => &e.message,
            Self::Response(e) => &e.message,
        }
    }

    pub fn error_id(&self) -> Option<u32> {
        match self {
            Self::Http(e) => e.error_id,
            Self::Response(e) => super::flags::HasErrorId::error_id(e),
        }
    }
}

impl std::fmt::Display for InBandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for InBandError {}

/// Build the classified error for an in-band failure frame, or `None` when the frame is not a
/// retryable in-band failure (in which case the caller keeps its existing handling and message).
///
/// `frame` is the decoded SSE `data:` payload, or the `{ error, response }` subset of one.
pub fn create_in_band_provider_error(frame: &Value) -> Option<InBandError> {
    let signal = read_in_band_signal(frame)?;
    let InBandSignal { status, code, detail } = signal;
    if let Some(status) = status.filter(|s| is_retryable_status(Some(*s))) {
        let message = format_in_band_message(status, detail.as_deref(), code.as_deref());
        return Some(InBandError::Http(
            ProviderHttpError::new(message, status, ProviderHttpErrorOptions { code, ..Default::default() })
                .attach(in_band_flags()),
        ));
    }
    if detail.is_none() && code.is_none() {
        return None;
    }
    // Keep the upstream code visible (`(<code>)`): it is real provider data and the same
    // convention the Anthropic provider already uses for its `(<errorType>)` suffix.
    let message = format!(
        "{}{}",
        detail.as_deref().unwrap_or(IN_BAND_DETAIL_PLACEHOLDER),
        code.as_deref().filter(|c| !c.is_empty()).map_or(String::new(), |c| format!(" ({c})"))
    );
    Some(InBandError::Response(
        ProviderResponseError::new(
            message,
            ProviderResponseErrorOptions { kind: Some(ProviderResponseErrorKind::Runtime), ..Default::default() },
        )
        .attach(in_band_flags()),
    ))
}

/// Build the classified error for a non-JSON SSE frame: gateways and reverse proxies that answer
/// `data: 429 Too Many Requests` or an HTML throttle page instead of an OpenAI envelope. `None`
/// when the text is not recognisable as a throttle, so genuinely malformed payloads keep failing loudly.
pub fn create_in_band_provider_error_from_text(text: &str) -> Option<InBandError> {
    let detail = read_in_band_detail_str(text)?;
    if !in_band_retryable_text().is_match(&detail) {
        return None;
    }
    let status = read_leading_status(Some(&detail));
    if let Some(status) = status.filter(|s| is_retryable_status(Some(*s))) {
        let message = format_in_band_message(status, Some(&detail), None);
        return Some(InBandError::Http(
            ProviderHttpError::new(message, status, ProviderHttpErrorOptions::default()).attach(in_band_flags()),
        ));
    }
    // A proxy status line with no machine code to preserve: report the upstream text verbatim
    // rather than padding it with wording of ours.
    Some(InBandError::Response(
        ProviderResponseError::new(
            detail,
            ProviderResponseErrorOptions { kind: Some(ProviderResponseErrorKind::Runtime), ..Default::default() },
        )
        .attach(in_band_flags()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::{is, retriable};
    use serde_json::json;

    fn http(e: &InBandError) -> &ProviderHttpError {
        match e {
            InBandError::Http(h) => h,
            InBandError::Response(r) => panic!("expected http error, got {r:?}"),
        }
    }

    #[test]
    fn nested_rate_limit_error_becomes_a_429_http_error() {
        let e = create_in_band_provider_error(&json!({"error": {"type": "rate_limit_error", "message": "Slow down"}})).unwrap();
        let h = http(&e);
        assert_eq!(h.status, 429);
        assert_eq!(h.message, "429 Slow down (rate_limit_error)");
        assert!(retriable(e.error_id(), None));
    }

    #[test]
    fn flat_status_and_string_status_fields() {
        let e = create_in_band_provider_error(&json!({"code": 429})).unwrap();
        assert_eq!(http(&e).message, "429 Provider returned an in-band provider error");
        let e = create_in_band_provider_error(&json!({"status": "503", "message": "busy"})).unwrap();
        assert_eq!(http(&e).status, 503);
    }

    #[test]
    fn non_retryable_codes_and_auth_statuses_fall_through() {
        assert!(create_in_band_provider_error(&json!({"error": {"code": "insufficient_quota", "message": "rate limit"}})).is_none());
        assert!(create_in_band_provider_error(&json!({"error": {"code": 401, "message": "unauthorized"}})).is_none());
        assert!(create_in_band_provider_error(&json!({"type": "response.output_text.delta"})).is_none());
        assert!(create_in_band_provider_error(&json!([1, 2])).is_none());
    }

    #[test]
    fn code_spellings_are_normalized_and_double_wrapped_errors_unwrap() {
        let e = create_in_band_provider_error(&json!({"error": {"code": "Throttling.AllocationQuota", "message": "x"}})).unwrap();
        assert_eq!(http(&e).status, 429);
        let e = create_in_band_provider_error(&json!({"error": {"error": {"code": "ThrottlingError", "message": "y"}}})).unwrap();
        assert_eq!(http(&e).status, 429);
    }

    #[test]
    fn throttle_wording_without_a_status_is_a_transient_response_error() {
        let e = create_in_band_provider_error(&json!({"message": "Please retry later"})).unwrap();
        assert!(matches!(e, InBandError::Response(_)));
        assert_eq!(e.message(), "Please retry later");
        assert!(is(e.error_id(), Flag::Transient));
        assert!(create_in_band_provider_error(&json!({"message": "bad input"})).is_none());
    }

    #[test]
    fn plain_text_frames() {
        let e = create_in_band_provider_error_from_text("429 Too Many Requests").unwrap();
        assert_eq!(http(&e).status, 429);
        assert_eq!(e.message(), "429 Too Many Requests");
        let html = create_in_band_provider_error_from_text("<html><body>503 Service Unavailable</body></html>").unwrap();
        assert_eq!(http(&html).status, 503);
        assert!(create_in_band_provider_error_from_text("chatcmpl-500321 done").is_none());
        let proxy = create_in_band_provider_error_from_text("Request was throttled").unwrap();
        assert!(matches!(proxy, InBandError::Response(_)));
    }
}
