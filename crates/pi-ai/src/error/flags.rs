//! Error classification bits. Port of the foundation of `src/error/flags.ts`.
//!
//! In TS an error carries its id as a non-enumerable `errorId` property set by `attach(error, id)`.
//! In Rust an error type exposes it through [`HasErrorId`].

use regex::Regex;
use std::sync::OnceLock;

use super::classes::STREAM_ENVELOPE_ERROR_PREFIX;
use super::rate_limit::{
    is_402_billing_cap_body, is_account_scoped_cap_text, is_dash_scope_token_limit_text, is_opaque_status_body,
    is_usage_limit_status, matches_usage_limit_text, parse_json_body, parse_rate_limit_reason, RateLimitReason,
};
use crate::pi_utils::fetch_retry::is_unexpected_socket_close_message;

/// Classification bits. A raw HTTP status (100-599) is also a valid id when `Flag::Class` is unset.
pub struct Flag;

#[allow(non_upper_case_globals)]
impl Flag {
    pub const Class: u32 = 0x1000;
    pub const ThinkingLoop: u32 = 0x0001_0000;
    pub const Transient: u32 = 0x0002_0000;
    pub const Timeout: u32 = 0x0004_0000;
    pub const UsageLimit: u32 = 0x0008_0000;
    pub const StaleResponsesItem: u32 = 0x0010_0000;
    pub const MalformedFunctionCall: u32 = 0x0020_0000;
    pub const ProviderFinishError: u32 = 0x0040_0000;
    pub const EmptyResponse: u32 = 0x0000_2000;
    pub const ContentBlocked: u32 = 0x0000_8000;
    /// Account-scoped provider policy denial that may succeed with another credential.
    pub const AccountPolicy: u32 = 0x0000_4000;
    pub const ContextOverflow: u32 = 0x0080_0000;
    pub const AuthFailed: u32 = 0x0100_0000;
    pub const SilentAbort: u32 = 0x0200_0000;
    pub const UserInterrupt: u32 = 0x0400_0000;
    pub const Abort: u32 = 0x0800_0000;
    /// Strict-tool rejection (400): grammar too large, schema too complex, or structured outputs unsupported by the model/endpoint.
    pub const Grammar: u32 = 0x1000_0000;
    /// Anthropic model/account does not support fast mode / the `speed` parameter.
    pub const FastModeUnsupported: u32 = 0x2000_0000;
    /// OAuth refresh failed definitively: the stored grant is dead, re-login required.
    pub const OAuthExpiry: u32 = 0x4000_0000;
    /// An Anthropic-compatible endpoint rejected the `cache_control` field with a 400, so
    /// prompt-cache breakpoints must be dropped for it.
    ///
    /// Every bit from `Class` up is already taken, so this claims a low bit instead. Those belong
    /// to the raw-status lane (an unclassified id is an HTTP status, 100-599, see `status_from_id`)
    /// and 0x800 is 2048, above every status value, so no raw status id can set it.
    pub const CacheControlUnsupported: u32 = 0x0000_0800;
    /// HTTP 413 byte/media rejection: token compaction cannot shrink bytes or media budgets (#9235).
    pub const PayloadRejected: u32 = 0x8000_0000;
}

pub const KIND_MASK: u32 = Flag::ThinkingLoop
    | Flag::Transient
    | Flag::Timeout
    | Flag::UsageLimit
    | Flag::StaleResponsesItem
    | Flag::MalformedFunctionCall
    | Flag::ProviderFinishError
    | Flag::EmptyResponse
    | Flag::ContentBlocked
    | Flag::AccountPolicy
    | Flag::ContextOverflow
    | Flag::AuthFailed
    | Flag::PayloadRejected
    | Flag::SilentAbort
    | Flag::UserInterrupt
    | Flag::Abort
    | Flag::Grammar
    | Flag::FastModeUnsupported
    | Flag::CacheControlUnsupported
    | Flag::OAuthExpiry;

const RETRIABLE_KINDS: u32 = Flag::Transient
    | Flag::UsageLimit
    | Flag::ThinkingLoop
    | Flag::StaleResponsesItem
    | Flag::ProviderFinishError
    | Flag::EmptyResponse;

/// What `attach(error, id)` does in TS: anything that can carry an `errorId`.
pub trait HasErrorId {
    fn error_id(&self) -> Option<u32>;
}

pub fn create(flags: &[u32]) -> u32 {
    let mut bits = 0;
    for f in flags {
        bits |= f;
    }
    bits | Flag::Class
}

pub fn is(id: Option<u32>, flag: u32) -> bool {
    (id.unwrap_or(0) & flag) != 0
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RetriableOptions {
    pub replay_unsafe: bool,
}

pub fn retriable(id: Option<u32>, opts: Option<RetriableOptions>) -> bool {
    if is(id, Flag::ContentBlocked) {
        return false;
    }
    if is(id, Flag::PayloadRejected) {
        return false;
    }
    if opts.map_or(false, |o| o.replay_unsafe) {
        return false;
    }
    if is(id, Flag::MalformedFunctionCall) {
        return true;
    }
    (id.unwrap_or(0) & RETRIABLE_KINDS) != 0
}

fn is_classified(id: Option<u32>) -> bool {
    (id.unwrap_or(0) & Flag::Class) != 0
}

/// An unclassified id is a raw HTTP status.
pub fn status_from_id(id: Option<u32>) -> Option<u32> {
    match id {
        Some(v) if v != 0 && !is_classified(id) => Some(v),
        _ => None,
    }
}

fn status_message_patterns() -> &'static [Regex; 5] {
    static P: OnceLock<[Regex; 5]> = OnceLock::new();
    P.get_or_init(|| {
        [
            Regex::new(r"(?i)\bstatus(?:_code)?[:=]\s*(\d{3})\b").unwrap(),
            Regex::new(r"(?i)\bstatus\s+(\d{3})\b").unwrap(),
            Regex::new(r"(?i)\bHTTP\s+(\d{3})\b").unwrap(),
            Regex::new(r"(?i)\b(?:error|failed)\s*[:=]?\s*(\d{3})\b").unwrap(),
            Regex::new(r"(?:^|\s)(\d{3})\s+(?:[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)").unwrap(),
        ]
    })
}

/// The message-text half of TS `status(error)`: the first status-looking number in `message`.
pub fn status_from_message(message: &str) -> Option<u32> {
    for pattern in status_message_patterns() {
        if let Some(code) = pattern.captures(message).and_then(|c| c[1].parse::<u32>().ok()) {
            if (100..=599).contains(&code) {
                return Some(code);
            }
        }
    }
    None
}

/// How the TS value that reached a classifier was shaped: only `Error` instances, plain strings
/// and bare objects differ for the `instanceof Error` checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorShape {
    Error,
    String,
    Object,
}

/// The pi-ai error classes the classifier singles out with `instanceof` / `name` checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass<'a> {
    Other,
    AwsCredentials,
    AnthropicConnectionTimeout,
    AnthropicConnection,
    CodexWebSocketTransport,
    CodexProviderStream { retryable: bool },
    ProviderHttp { status: u32, code: Option<&'a str> },
    /// `OAuthError` with `kind === "token-refresh"`.
    OAuthTokenRefresh,
}

/// The shape TS `classify(error: unknown)` walks: an error, a message-shaped object
/// (`AssistantMessage`), or a bare string, with an optional cause chain.
pub trait ErrorLike {
    /// The numeric `errorId` property (set by `attach`), when present.
    fn error_id_prop(&self) -> Option<u32> {
        None
    }
    /// The string `code` property.
    fn code(&self) -> Option<&str> {
        None
    }
    /// `status`, `statusCode`, `errorStatus` or `response.status` when present.
    fn own_status(&self) -> Option<u32> {
        None
    }
    /// The `message` property (or the string itself for a string error).
    fn message(&self) -> Option<&str> {
        None
    }
    /// The `errorMessage` property of an `AssistantMessage`-shaped object.
    fn error_message(&self) -> Option<&str> {
        None
    }
    /// The `errorClassificationMessage` property of an `AssistantMessage`-shaped object.
    fn error_classification_message(&self) -> Option<&str> {
        None
    }
    fn cause(&self) -> Option<&dyn ErrorLike> {
        None
    }
    /// The `errors` array of an `AggregateError`.
    fn errors(&self) -> Vec<&dyn ErrorLike> {
        Vec::new()
    }
    /// The `name` property (e.g. `AbortError`).
    fn name(&self) -> Option<&str> {
        None
    }
    /// The `headers` property (or `response.headers`), when the error carries one.
    fn headers(&self) -> Option<&reqwest::header::HeaderMap> {
        None
    }
    fn shape(&self) -> ErrorShape {
        ErrorShape::Error
    }
    fn class(&self) -> ErrorClass<'_> {
        ErrorClass::Other
    }
}

/// A bare string error, or the `{ message }` object TS passes to `status()`.
#[derive(Debug, Clone)]
pub struct TextError<'a>(pub &'a str);

impl ErrorLike for TextError<'_> {
    fn message(&self) -> Option<&str> {
        Some(self.0)
    }
    fn shape(&self) -> ErrorShape {
        ErrorShape::String
    }
}

/// A bare `{ message }` object (as opposed to [`TextError`], a string).
#[derive(Debug, Clone)]
pub struct MessageObject<'a>(pub &'a str);

impl ErrorLike for MessageObject<'_> {
    fn message(&self) -> Option<&str> {
        Some(self.0)
    }
    fn shape(&self) -> ErrorShape {
        ErrorShape::Object
    }
}

fn status_internal(error: &dyn ErrorLike, depth: u32) -> Option<u32> {
    if depth > 2 {
        return None;
    }
    if let Some(s) = error.own_status().filter(|s| (100..=599).contains(s)) {
        return Some(s);
    }
    if let Some(cause) = error.cause() {
        if let Some(nested) = status_internal(cause, depth + 1) {
            return Some(nested);
        }
    }
    error.message().and_then(status_from_message)
}

/// HTTP status of an error, from its own fields, its cause chain, or its message text.
pub fn status(error: &dyn ErrorLike) -> Option<u32> {
    status_internal(error, 0)
}

// ---------------------------------------------------------------------------
// Text patterns
// ---------------------------------------------------------------------------

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            C.get_or_init(|| Regex::new($pat).expect("static pattern"))
        }
    };
}

macro_rules! lazy_re_list {
    ($name:ident, [$($pat:expr),+ $(,)?]) => {
        fn $name() -> &'static Vec<Regex> {
            static C: OnceLock<Vec<Regex>> = OnceLock::new();
            C.get_or_init(|| vec![$(Regex::new($pat).expect("static pattern")),+])
        }
    };
}

lazy_re_list!(
    context_overflow_evidence_patterns,
    [
        r"(?i)prompt is too long", // Anthropic
        r"(?i)input is too long for requested model", // Amazon Bedrock
        r"(?i)exceeds the context window", // OpenAI (Completions & Responses API)
        r"(?i)input token count.*exceeds the maximum", // Google (Gemini)
        r"(?i)maximum prompt length is \d+", // xAI (Grok)
        r"(?i)reduce the length of the messages", // Groq
        r"(?i)maximum context length is \d+ tokens", // OpenRouter (all backends)
        r"(?i)exceeds the available context size", // llama.cpp server
        r"(?i)\bprompt\s*\(\s*\d+\s+tokens\s*\)\s*\+\s*max\s+tokens\s*\(\s*\d+\s*\)\s+exceeds\s+the\s+context\s*\(\s*\d+\s*\)", // Strata server
        r"(?i)\bprompt\s*\(\s*\d+\s+tokens\s*\)\s+leaves\s+no\s+room\s+to\s+answer\s+in\s+the\s+context\s*\(\s*\d+\s*\)", // Strata server
        r"(?i)requested tokens?.*exceed.*context (window|length|size)", // llama.cpp / OpenAI-compatible local servers
        r"(?i)context (window|length|size).*(exceeded|overflow|too small)", // Generic local server variants
        r"(?i)(prompt|input).*(too long|too large).*(context|n_ctx)", // llama.cpp phrasing variants
        r"(?i)requested tokens?.*(exceeds?|greater than).*(n_ctx|context)", // llama.cpp n_ctx variants
        r"(?i)greater than the context length", // LM Studio
        r"(?i)context window exceeds limit", // MiniMax
        r"(?i)exceeded model token limit", // Kimi For Coding
        r"(?i)context[_ ]length[_ ]exceeded", // Generic fallback
        r"(?i)too many tokens", // Generic fallback
        r"(?i)token limit exceeded", // Generic fallback
        r"(?i)request_too_large[^\n]*\btokens?\b",
        r"(?i)\btokens?\b[^\n]*request_too_large",
        r"(?i)model_context_window_exceeded", // z.ai non-standard finish_reason surfaced as error text
        r"(?i)prompt filled the context window", // Ollama OpenAI-compatible empty length completion
        r"(?i)exceeds the limit of \d+ tokens?\b",
        r"(?i)chat history exceeds the \d+-message limit", // Provider message-count cap
    ]
);
// Numeric limit pattern: also matches media budgets, so must never veto a payload flag (#9235).
lazy_re!(generic_limit_overflow, r"(?i)exceeds the limit of \d+");

/// Token-context evidence only: excludes the generic numeric limit which media budgets also match (#9235).
fn has_token_context_overflow_evidence(text: &str) -> bool {
    context_overflow_evidence_patterns().iter().any(|p| p.is_match(text))
}

/// Checks every cause-chain link; a wrapper whose nested 413 names the token budget stays overflow-only (#9235).
fn has_cause_token_context_overflow_evidence(error: &dyn ErrorLike) -> bool {
    let mut link: Option<&dyn ErrorLike> = Some(error);
    while let Some(l) = link {
        if let Some(m) = l.message() {
            if has_token_context_overflow_evidence(m) {
                return true;
            }
        }
        link = l.cause();
    }
    false
}

lazy_re!(overflow_no_body, r"(?i)\b4(00|13)\s*(status code)?\s*\(no body\)");
// Bare `413 (no body)` deliberately dual-flagged; maintenance arbitrates against local headroom (#9235).
lazy_re_list!(
    payload_rejection_patterns,
    [
        r"(?i)\b413\s*(?:status code\s*)?\(no body\)",
        r"(?i)\b413\b[^.\n]{0,120}\b(?:request|payload|entity|body)\b[^.\n]{0,60}\b(?:exceed|too large|limit)",
        r"(?i)request_too_large",
        r"(?i)(?:payload|entity) too large",
        r"(?i)request exceeds the maximum (?:size|number of bytes)",
    ]
);

fn matches_payload_rejection_text(text: &str) -> bool {
    if !payload_rejection_patterns().iter().any(|p| p.is_match(text)) {
        return false;
    }
    // Token-context evidence vetoes; only token wording counts: generic numeric limits also match media budgets.
    !has_token_context_overflow_evidence(text)
}

lazy_re!(timeout_pattern, r"(?i)\b(?:operation\s+)?timed?\s*out\b|\btimeout\b|\bstream stall\b");
lazy_re!(transient_envelope, r"(?i)anthropic stream envelope error:");
lazy_re!(transient_envelope_truncation, r"(?i)before message_(?:start|delta|stop)");
lazy_re!(stream_read_error_pattern_re, r"(?i)stream[_ -]?read[_ -]?error");
/// Python h2/httpx diagnostics forwarded through provider or proxy error events.
lazy_re!(python_http2_stream_reset_re, r"<StreamReset stream_id:\d+, error_code:(?:2|7), remote_reset:True>");
/// Python h11/httpx EOF while reading an HTTP/1.1 chunked response body.
lazy_re!(
    python_http_incomplete_chunk_re,
    r"peer closed connection without sending complete message body \(incomplete chunked read\)"
);
lazy_re!(codex_http_body_read_error_re, r"(?i)\btransport error reading codex response body\b");

pub fn stream_read_error_pattern() -> &'static Regex {
    stream_read_error_pattern_re()
}
pub fn python_http2_stream_reset_pattern() -> &'static Regex {
    python_http2_stream_reset_re()
}
pub fn python_http_incomplete_chunk_pattern() -> &'static Regex {
    python_http_incomplete_chunk_re()
}
/// reqwest body-frame failures forwarded by the Codex HTTP proxy.
pub fn codex_http_body_read_error_pattern() -> &'static Regex {
    codex_http_body_read_error_re()
}

lazy_re!(responses_request_body_read_timeout, r"(?i)\btimed out reading request body\b");

/// Exact HTTP request-body-read timeout diagnostic.
pub fn is_request_body_read_timeout(status: Option<u32>, message: Option<&str>) -> bool {
    status == Some(408) && responses_request_body_read_timeout().is_match(message.unwrap_or(""))
}

/// The error fields `is_responses_request_body_read_timeout` / `is_codex_steer_rejection` read.
#[derive(Debug, Clone, Default)]
pub struct ErrorFacts<'a> {
    pub api: Option<&'a str>,
    pub error_status: Option<u32>,
    pub error_message: Option<&'a str>,
    pub request_body_read_timeout_full_replay: bool,
}

/// Exact pre-output Responses 408 that needs a changed-request recovery path.
pub fn is_responses_request_body_read_timeout(message: &ErrorFacts<'_>) -> bool {
    message.api == Some("openai-responses")
        && message.request_body_read_timeout_full_replay
        && is_request_body_read_timeout(message.error_status, message.error_message)
}

/// Codex code for the experimental native turn lane refusing a mid-response `response.steer`; the
/// server then drops the in-flight response and closes the socket.
pub const CODEX_NATIVE_LANE_STEER_REJECTED_CODE: &str = "unsupported_native_inflight_message";

/// A Codex turn the native turn lane dropped because omp steered it. The rejection answers our own
/// `response.steer`, not the model's health: the provider stops steering the session, so the same
/// model replays cleanly.
pub fn is_codex_steer_rejection(api: Option<&str>, error_message: Option<&str>) -> bool {
    api == Some("openai-codex-responses")
        && error_message.map_or(false, |m| m.contains(&format!("code={CODEX_NATIVE_LANE_STEER_REJECTED_CODE}")))
}

/// Transport-failure wording that marks an error transient. Covers diagnostics forwarded verbatim
/// by gateways/proxies in other runtimes, e.g. undici's "other side closed" and Go's
/// `net.ErrClosed` ("use of closed network connection", #15385). Also covers OMP's own
/// network-failure sentences from `network_failure_message` (offline / unreachable / refused /
/// TLS), which must stay at least as retryable as the bare `Connection error.` text they replaced.
///
/// The TS lookahead `(?=[:\s]|$)` after `auth-gateway 5xx` is written as a consuming group: this
/// pattern is only ever used for a boolean match, so the two are equivalent.
pub fn transient_transport_pattern() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no[_ -]?capacity|(?:high|peak)[ _-]?demand|(?:at|over|insufficient)[ _-]?capacity|capacity[ _-]?(?:exceeded|exhausted)|peak[ _-]?load)\b|overloaded|provider.?returned.?error|rate.?limit|too many requests|auth-gateway\s+5\d{2}(?:[:\s]|$)|\b(?:429|500|502|503|504)\b|service.?unavailable|temporar(?:y|ily)[ _-]?unavailable|server.?error|internal.?error|retry your request|network.?error|connection.?error|connection.?refused|unable.?to.?connect\.\s*is the computer able to access the url\?|other side closed|use of closed network connection|fetch failed|upstream.?connect|upstream.?request.?failed|reset before headers|socket hang up|timed? out|timeout|terminated|retry delay|stream stall|no error details in response|HTTP2(?:StreamReset|RefusedStream|EnhanceYourCalm)|nghttp2_(?:internal_error|refused_stream)|stream closed with error code nghttp2_(?:internal_error|refused_stream)|\bE(?:TIMEOUT|NOTFOUND|NETUNREACH|HOSTUNREACH)\b|\bFailedToOpenSocket\b|no internet connection|couldn.?t reach|couldn.?t connect to|tls certificate verification failed|malformed.?function.?call",
        )
        .expect("static pattern")
    })
}

lazy_re!(
    auth_failure_pattern,
    r"(?i)\b(?:401|403|unauthorized|forbidden|authentication|auth[_ ]?unavailable|no auth available|(?:invalid|no)[_ ]?api[_ ]?key)\b"
);
lazy_re!(
    malformed_function_call_pattern,
    r"(?i)\b(?:malformed[_. ]?(?:function[_. ]?call|response)|unexpected[_. ]?tool[_. ]?call)\b"
);
lazy_re!(provider_finish_error_pattern, r"(?i)\bProvider (?:returned error finish_reason|finish_reason:\s*error)\b");
lazy_re!(empty_response_pattern, r"(?i)\bthought-only response without final output\b");
/// Gemini's MALFORMED_RESPONSE finish is a backend fault: the same request can succeed. Kept
/// narrow so other "malformed response" errors (e.g. an image API's) stay terminal.
lazy_re!(malformed_response_finish_pattern, r"(?i)\bfinish reason:\s*MALFORMED_RESPONSE\b");
lazy_re!(content_filter_pattern, r"(?i)\b(?:incomplete:\s*)?content_filter\b");
lazy_re!(account_policy_pattern, r"(?i)\bcyber_policy\b|trusted access for cyber");

pub fn anthropic_account_policy_pattern() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:oauth_not_allowed_for_organization|permission_error)\b|\bOAuth authentication is currently not allowed for this organization\b",
        )
        .expect("static pattern")
    })
}

/// Whether an error message represents an Anthropic account-scoped permission/policy denial.
pub fn is_anthropic_account_policy_text(text: &str, provider: Option<&str>, status_arg: Option<u32>) -> bool {
    if provider.map_or(false, |p| p != "anthropic") {
        return false;
    }
    let status_candidate = status_arg.or_else(|| if text.is_empty() { None } else { status(&TextError(text)) });
    if status_candidate.map_or(false, |s| s != 403) {
        return false;
    }
    anthropic_account_policy_pattern().is_match(text)
}

lazy_re!(
    immutable_latest_thinking_pattern,
    r"(?is)messages\.\d+\.content\.\d+.*\b(?:thinking|redacted_thinking)\b.*\blatest assistant message cannot be modified\b"
);

/// Anthropic's rejection of a replayed latest assistant turn whose `thinking` or
/// `redacted_thinking` blocks no longer verify. Resending the same turn fails the same way; only a
/// request that changes that turn's reasoning can succeed.
pub fn is_immutable_latest_thinking_text(text: &str) -> bool {
    immutable_latest_thinking_pattern().is_match(text)
}

lazy_re!(
    codex_chatgpt_account_model_policy,
    r#"(?i)\bThe ['"]([^'"\r\n]+)['"] model is not supported when using Codex with a ChatGPT account\."#
);
const CODEX_CHATGPT_ACCOUNT_MODEL_MAX_LENGTH: usize = 256;
lazy_re!(cursor_plan_policy_marker, r"(?i)\bERROR_RATE_LIMITED_CHANGEABLE\b");
lazy_re!(
    cursor_plan_policy,
    r"(?i)\bNamed models unavailable\b|\bModel unavailable on\b|\bFree plans can only use\b"
);

fn is_cursor_plan_policy_text(text: &str) -> bool {
    cursor_plan_policy_marker().is_match(text) && cursor_plan_policy().is_match(text)
}

fn normalize_codex_chatgpt_account_policy_model(model_id: Option<&str>) -> Option<String> {
    let model_id = model_id?;
    let bare = match model_id.rfind('/') {
        None => model_id,
        Some(i) => &model_id[i + 1..],
    };
    let bare = bare.trim().to_lowercase();
    if bare.is_empty() || bare.len() > CODEX_CHATGPT_ACCOUNT_MODEL_MAX_LENGTH || bare.contains('\0') {
        return None;
    }
    Some(bare)
}

fn codex_chatgpt_account_policy_model_from_text(text: &str) -> Option<String> {
    let model_id = codex_chatgpt_account_model_policy().captures(text)?.get(1)?.as_str().trim().to_owned();
    normalize_codex_chatgpt_account_policy_model(Some(&model_id)).map(|_| model_id)
}

fn is_codex_chatgpt_account_policy_text(text: &str, provider: Option<&str>, model_id: Option<&str>) -> bool {
    if provider != Some("openai-codex") {
        return false;
    }
    let denied = codex_chatgpt_account_policy_model_from_text(text);
    let denied_identity = normalize_codex_chatgpt_account_policy_model(denied.as_deref());
    let requested_identity = normalize_codex_chatgpt_account_policy_model(model_id);
    denied_identity.is_some() && denied_identity == requested_identity
}

lazy_re_list!(
    stale_response_item_patterns,
    [r#"(?i)\bItem with id ['"][^'"]+['"] not found\.?"#, r"(?i)previous[ _]?response"]
);
lazy_re!(stale_response_item_detail, r"(?i)not[ _]?found|invalid|expired|stale|zero[ _-]?data[ _-]?retention");

/// Local llama.cpp / Ollama deterministic tool-call argument JSON parse failure. The model emitted
/// invalid JSON in a tool call and the server returned HTTP 500 with this exact text: replaying the
/// same prompt yields the same malformed output, so callers strip `Flag::Transient` when this matches.
pub fn llama_cpp_tool_call_parse_pattern() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| {
        Regex::new(r"(?i)failed to parse tool call arguments as json|\[json\.exception\.parse_error\.101\]")
            .expect("static pattern")
    })
}

// Fireworks (and other OpenAI-compat backends) can abort mid-generation with an HTTP 400
// `invalid_request_error` whose body reports a model-side numerical fault: "Floating point NaN
// (not-a-number) is detected in generation". Despite the request-validation wrapper this is a
// decode-time logits overflow, not a bad request: a byte-identical replay succeeds. Kept narrow.
lazy_re!(generation_nan_pattern, r"(?is)floating[ _-]?point nan\b.*\bdetected in generation");
// Aliyun DashScope's compatible-mode multimodal gateway intermittently fails perfectly valid
// inline image payloads with a validation-flavored 400. A byte-identical replay succeeds. Kept
// narrow to these two gateway phrasings so genuine request-validation 400s stay terminal.
pub fn aliyun_mm_gateway_pattern() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| {
        Regex::new(r"(?i)provided url does not appear to be valid|download multimodal file timed out")
            .expect("static pattern")
    })
}
// Anthropic strict-tool grammar too large / schema too complex (400 invalid_request_error).
// Feature-gated deployments (Azure Foundry, Baseten, ...) reject `strict: true` tools outright when
// the hosted model lacks structured outputs, without an invalid_request_error wrapper.
lazy_re!(grammar_too_large, r"(?i)compiled grammar");
lazy_re!(grammar_too_large_detail, r"(?i)too large");
lazy_re!(schema_too_complex, r"(?i)schema");
lazy_re!(schema_too_complex_detail, r"(?i)too complex");
lazy_re!(schema_compile, r"(?i)compil");
lazy_re!(invalid_request, r"(?i)invalid_request_error");
lazy_re!(structured_outputs, r"(?i)structured[_ -]?outputs?");
lazy_re!(
    feature_not_supported,
    r"(?i)not (?:supported|available|enabled)|unsupported|does(?: not|n'?t) support"
);
lazy_re!(anthropic_strict_field, r"(?i)\btools\.\d+\.custom\.strict\b");
lazy_re!(extra_inputs_not_permitted, r"(?i)extra inputs? (?:are|is) not permitted");
// Upstream strict-schema validation surfaced through a translating gateway (Vercel AI Gateway).
lazy_re!(strict_tool_schema_rejection, r"(?i)invalid schema for function|invalid tool parameters schema");
// Anthropic fast-mode unsupported: 400 rejecting `speed`, or 429 rate_limit_error because the
// account lacks the extra-usage entitlement fast mode requires.
lazy_re!(fast_mode_speed_param, r"(?i)\bspeed\b");
lazy_re!(fast_mode_not_supported, r"(?i)not support");
lazy_re!(fast_mode_rate_limit, r"(?i)rate_limit_error");
lazy_re!(fast_mode_entitlement, r"(?i)fast mode");
// Anthropic-compatible proxies that do not implement prompt caching reject the `cache_control`
// field itself with a 400 naming it. Requires rejection wording so a 400 that merely mentions the
// field for another reason stays terminal.
//
// The trigger is deliberately a bare mention. Whether such a 400 refused the field or one option
// *under* it (`ttl`, `scope`) is not decidable from the message; the retry ladder in
// `providers/anthropic.ts` asks the endpoint the rest by replaying.
lazy_re!(cache_control_field, r"(?i)\bcache_control\b");
lazy_re!(
    cache_control_rejection,
    r"(?i)\bunexpected\b|\bunrecognized\b|\bnot permitted\b|\bnot allowed\b|\bnot recognized\b|\bnot supported\b|\bunsupported\b|\binvalid[_ ]field\b|\bextra (?:inputs?|fields?)\b"
);
// The bare-word gate above reads prose, and `_` is a word character, so it misses every
// snake_cased error code. Strict decoders and OpenAI-compatible validators report a refused
// *member* either as a negation or as a bare adjective. Both families stay anchored to a
// schema-member noun one joiner away, and that anchor is the false-positive bound.
lazy_re!(
    cache_control_member_refusal,
    r"(?i)\b(?:un(?:known|permitted|supported|recognized|expected)|invalid|bad|not (?:an? )?(?:known|valid|accepted|supported|permitted|allowed|recognized))[_ -](?:field|name|param(?:eter)?|argument|key|propert(?:y|ies)|attribute|member|option|input)s?\b"
);
// Anthropic rejects a breakpoint on an empty text block with a 400 that names `cache_control` too.
// That is a content-shape fault: dropping every breakpoint neither fixes it nor proves the
// endpoint lacks prompt caching.
lazy_re!(
    cache_control_empty_text,
    r"(?i)empty text block|text content block.*non-(?:empty|whitespace)|text.{0,40}must (?:be non-empty|not be empty)"
);
// Definitive OAuth refresh failure: the stored grant/client is dead.
lazy_re!(
    oauth_definitive_failure,
    r"(?i)invalid_grant|invalid_token|unauthorized_client|\brevoked\b|refresh[\s_]?token.*expired"
);
lazy_re!(
    oauth_transient_failure,
    r"(?i)timeout|network|fetch failed|ECONN(?:REFUSED|RESET)|ETIMEDOUT|ETIMEOUT|ENOTFOUND|ENETUNREACH|EHOSTUNREACH|FailedToOpenSocket|EAI_AGAIN|socket hang up|\b(?:408|425|429|5\d{2})\b|rate.?limit|too many requests|temporar|unavailable|forbidden|permission_denied|cloudflare|captcha"
);
lazy_re!(oauth_http_auth, r"\b401\b");

fn matches_strict_tools_rejection(message: &str, error_status: Option<u32>) -> bool {
    if error_status != Some(400) {
        return false;
    }
    if anthropic_strict_field().is_match(message) && extra_inputs_not_permitted().is_match(message) {
        return true;
    }
    if structured_outputs().is_match(message) && feature_not_supported().is_match(message) {
        return true;
    }
    if strict_tool_schema_rejection().is_match(message) {
        return true;
    }
    if !invalid_request().is_match(message) {
        return false;
    }
    let grammar_too_large_hit = grammar_too_large().is_match(message) && grammar_too_large_detail().is_match(message);
    let schema_too_complex_hit = schema_too_complex().is_match(message)
        && schema_too_complex_detail().is_match(message)
        && schema_compile().is_match(message);
    grammar_too_large_hit || schema_too_complex_hit
}

fn matches_fast_mode_unsupported(message: &str, error_status: Option<u32>) -> bool {
    if error_status != Some(400) && error_status != Some(429) {
        return false;
    }
    if error_status == Some(400)
        && invalid_request().is_match(message)
        && fast_mode_speed_param().is_match(message)
        && fast_mode_not_supported().is_match(message)
    {
        return true;
    }
    error_status == Some(429) && fast_mode_rate_limit().is_match(message) && fast_mode_entitlement().is_match(message)
}

fn matches_cache_control_rejection(message: &str, error_status: Option<u32>) -> bool {
    if error_status != Some(400) {
        return false;
    }
    if !cache_control_field().is_match(message) {
        return false;
    }
    if cache_control_empty_text().is_match(message) {
        return false;
    }
    cache_control_rejection().is_match(message) || cache_control_member_refusal().is_match(message)
}

/// Whether an OAuth refresh error message means the grant is definitively dead.
pub fn is_oauth_expiry(error_message: &str) -> bool {
    if oauth_definitive_failure().is_match(error_message) {
        return true;
    }
    oauth_http_auth().is_match(error_message) && !oauth_transient_failure().is_match(error_message)
}

const ERROR_KIND_LABELS: [(u32, &str); 16] = [
    (Flag::ThinkingLoop, "thinking-loop"),
    (Flag::Transient, "transient"),
    (Flag::Timeout, "timeout"),
    (Flag::UsageLimit, "usage-limit"),
    (Flag::StaleResponsesItem, "stale-responses-item"),
    (Flag::MalformedFunctionCall, "malformed-function-call"),
    (Flag::ProviderFinishError, "provider-finish-error"),
    (Flag::EmptyResponse, "empty-response"),
    (Flag::ContentBlocked, "content-blocked"),
    (Flag::AccountPolicy, "account-policy"),
    (Flag::ContextOverflow, "context-overflow"),
    (Flag::PayloadRejected, "payload-rejected"),
    (Flag::AuthFailed, "auth-failed"),
    (Flag::SilentAbort, "silent-abort"),
    (Flag::UserInterrupt, "user-interrupt"),
    (Flag::Abort, "abort"),
];

pub fn is_stream_read_error_text(text: &str) -> bool {
    stream_read_error_pattern().is_match(text)
}

/// Persisted-text form of [`is_stream_envelope_error`]: recognizes the prefix-tagged envelope
/// diagnostic on an aborted turn's `errorMessage` / `stopDetails.explanation` so loop-level salvage
/// can classify it after the original `Error` instance is gone.
pub fn is_stream_envelope_error_text(text: &str) -> bool {
    text.contains(STREAM_ENVELOPE_ERROR_PREFIX)
}

/// Persisted-text form of a truncated stream envelope: an `Anthropic stream envelope error:`
/// diagnostic naming the frame the stream never reached (`before message_start|delta|stop`). Covers
/// both a connection that dies inside an open envelope and an in-band `error` frame that a gateway
/// or proxy substitutes for the cut, so turn recovery can classify the family by its envelope marker
/// instead of per-gateway wording.
pub fn is_stream_envelope_truncation_text(text: &str) -> bool {
    transient_envelope().is_match(text) && transient_envelope_truncation().is_match(text)
}

fn is_transient_error_text(text: &str) -> bool {
    is_unexpected_socket_close_message(text)
        || is_stream_read_error_text(text)
        || python_http2_stream_reset_pattern().is_match(text)
        || python_http_incomplete_chunk_pattern().is_match(text)
        || is_stream_envelope_truncation_text(text)
        || transient_transport_pattern().is_match(text)
}

fn is_timeout_text(text: &str) -> bool {
    timeout_pattern().is_match(text)
}

fn is_auth_failure_text(text: &str) -> bool {
    auth_failure_pattern().is_match(text)
}

fn is_stale_responses_text(text: &str) -> bool {
    let p = stale_response_item_patterns();
    p[0].is_match(text) || (p[1].is_match(text) && stale_response_item_detail().is_match(text))
}

fn matches_overflow_text(text: &str) -> bool {
    has_token_context_overflow_evidence(text)
        || generic_limit_overflow().is_match(text)
        || overflow_no_body().is_match(text)
}

/// A 4xx the provider rejected as a deterministic client error: every 4xx except 408 (Request
/// Timeout) and 429 (Too Many Requests), the retryable pair. Mirrors the 4xx policy in
/// `is_provider_retryable_error`: such a request replays identically, so a transient signal riding
/// on it (e.g. a truncation phrase in the body) must not flip it to retryable.
fn is_terminal_client_error_status(status: Option<u32>) -> bool {
    status.map_or(false, |s| (400..500).contains(&s) && s != 408 && s != 429)
}

fn classify_text(
    error_message: Option<&str>,
    error_status: Option<u32>,
    prior_token_overflow_evidence: bool,
    api: Option<&str>,
    provider: Option<&str>,
    model_id: Option<&str>,
) -> u32 {
    let mut kinds: u32 = 0;
    let error_message = error_message.filter(|m| !m.is_empty());
    if let Some(msg) = error_message {
        let status_clean: Option<u32> = match error_status {
            Some(s) if s != 0 => Some(s),
            _ => status(&TextError(msg)),
        };
        // A 429/402 is a throughput or quota decision, never window excess, even when worded like
        // one (Bedrock: "429 Too many tokens, please wait..."): overflow would skip retry and the
        // fallback chain for a compaction that cannot clear a rate cap.
        if matches_overflow_text(msg) && !is_usage_limit_status(status_clean) {
            kinds |= Flag::ContextOverflow;
        }
        if matches_payload_rejection_text(msg) {
            kinds |= Flag::PayloadRejected;
        }
        if malformed_function_call_pattern().is_match(msg) {
            kinds |= Flag::MalformedFunctionCall;
        }
        if provider_finish_error_pattern().is_match(msg) {
            kinds |= Flag::ProviderFinishError;
        }
        if empty_response_pattern().is_match(msg) {
            kinds |= Flag::EmptyResponse | Flag::Transient;
        }
        if malformed_response_finish_pattern().is_match(msg) {
            kinds |= Flag::Transient;
        }
        if content_filter_pattern().is_match(msg) {
            kinds |= Flag::ContentBlocked;
        }

        if account_policy_pattern().is_match(msg)
            || is_codex_chatgpt_account_policy_text(msg, provider, model_id)
            || (provider == Some("cursor") && is_cursor_plan_policy_text(msg))
        {
            kinds |= Flag::AccountPolicy | Flag::ContentBlocked;
        }
        if is_anthropic_account_policy_text(msg, provider, status_clean) {
            kinds |= Flag::AccountPolicy;
        }
        if is_auth_failure_text(msg) {
            kinds |= Flag::AuthFailed;
        }
        let is_opaque = is_opaque_status_body(msg);

        let is_limit_status = is_usage_limit_status(status_clean);
        let reason = parse_rate_limit_reason(msg);
        let is_402_billing_cap = status_clean == Some(402) && is_402_billing_cap_body(Some(msg));
        // Concurrency caps (e.g. Vertex "Online prediction concurrent requests quota exceeded") are
        // shed-and-backoff, not credential-rotatable: exclude them even when the quota-worded
        // phrasing matches the generic usage-limit text matcher, which would otherwise set
        // `Flag::UsageLimit` and burn a healthy sibling credential. HTTP 402 is excluded from this
        // gate: a 402 whose body merely mentions concurrency still classifies as a usage limit.
        let concurrency_excluded = reason == RateLimitReason::ConcurrentLimit && status_clean != Some(402);
        if !concurrency_excluded
            && (is_402_billing_cap
                || matches_usage_limit_text(msg)
                || ((status_clean == Some(403) || status_clean.is_none()) && is_account_scoped_cap_text(msg))
                || (is_limit_status && (is_opaque || reason == RateLimitReason::QuotaExhausted)))
        {
            kinds |= Flag::UsageLimit;
        }
        // Transport/timeout/truncation wording that rides on a terminal 4xx (e.g. a "400
        // unexpected EOF" malformed request, or a region/entitlement denial whose body carries
        // `type=server_error`) describes a deterministic client error that replays identically, so
        // keep it terminal: the same 4xx policy as `is_provider_retryable_error`. `classify`
        // carries the outer terminal status down the cause chain so a wrapped phrase
        // (ProviderHttpError 400 -> cause "unexpected EOF") is caught here too.
        if !is_terminal_client_error_status(status_clean) {
            if is_timeout_text(msg) {
                kinds |= Flag::Transient | Flag::Timeout;
            } else if is_transient_error_text(msg) {
                kinds |= Flag::Transient;
            }
            // A stream truncation, transport-level stream drop, or forwarded Codex HTTP body-read
            // failure may not match the transport pattern. Flag it explicitly so `retriable` and the
            // turn-recovery layer treat it as retryable, matching the provider retry path. Separate
            // `if` (not chained onto the else-if) so a timeout whose text also reads as a
            // truncation keeps `Flag::Timeout` alongside `Flag::Transient`. The string arm applies
            // the strict STREAM_PARSE_DIAGNOSTIC_PATTERN.
            if is_transient_stream_parse_error(&TextError(msg))
                || is_transient_stream_drop_error(&TextError(msg))
                || codex_http_body_read_error_pattern().is_match(msg)
            {
                kinds |= Flag::Transient;
            }
        }
        // A concurrency cap is transient (shed-and-backoff). The bare wording need not match the
        // transport pattern, so flag it explicitly to keep `retriable` from treating the
        // temporary cap as terminal.
        if reason == RateLimitReason::ConcurrentLimit {
            kinds |= Flag::Transient;
        }
        if (api == Some("openai-responses") || api == Some("openai-codex-responses")) && is_stale_responses_text(msg) {
            kinds |= Flag::StaleResponsesItem;
        }
        // Retryable like the provider's own classification of the code, so a message reclassified
        // from its text alone still retries.
        if is_codex_steer_rejection(api, Some(msg)) {
            kinds |= Flag::Transient;
        }

        // Fireworks mid-generation NaN 400 is a model-side decode fault, not a bad request; a
        // byte-identical replay succeeds, so treat it as transient.
        if status_clean == Some(400) && generation_nan_pattern().is_match(msg) {
            kinds |= Flag::Transient;
        }
        // Same shape for Aliyun's multimodal gateway: a flaky 400 on valid inline images.
        if status_clean == Some(400) && aliyun_mm_gateway_pattern().is_match(msg) {
            kinds |= Flag::Transient;
        }
        if matches_strict_tools_rejection(msg, status_clean) {
            kinds |= Flag::Grammar;
        }
        if matches_fast_mode_unsupported(msg, status_clean) {
            kinds |= Flag::FastModeUnsupported;
        }
        if matches_cache_control_rejection(msg, status_clean) {
            kinds |= Flag::CacheControlUnsupported;
        }
    }
    // Status-only 413: infer PayloadRejected unless prior classification carries token-context evidence (#9235).
    let status_evidence: Option<u32> = match error_status {
        Some(s) if s != 0 => Some(s),
        _ => error_message.and_then(|m| status(&TextError(m))),
    };
    if status_evidence == Some(413)
        && !prior_token_overflow_evidence
        && !error_message.map_or(false, has_token_context_overflow_evidence)
    {
        kinds |= Flag::PayloadRejected;
    }
    if status_evidence == Some(402) && error_message.map_or(true, is_opaque_status_body) {
        kinds |= Flag::UsageLimit;
    }
    if kinds != 0 {
        return create(&[kinds]);
    }
    let fallback_status = status_evidence;
    if fallback_status == Some(401) || fallback_status == Some(403) {
        return create(&[Flag::AuthFailed]);
    }
    fallback_status.unwrap_or(0)
}

/// Classify an error (walking its cause chain) into a flag id. A wrapper that carried no
/// classification falls back to its HTTP status, or 0.
pub fn classify(error: &dyn ErrorLike, api: Option<&str>) -> u32 {
    let mut kinds: u32 = 0;
    let cause_token_evidence = has_cause_token_context_overflow_evidence(error);
    let mut link: Option<&dyn ErrorLike> = Some(error);
    // A terminal 4xx on an outer link governs its own cause diagnostics: a wrapped truncation or
    // transport phrase is describing why the deterministic request failed, not an independently
    // retryable transport fault. Carry it down so the terminal-4xx guard in `classify_text` sees it
    // on the status-less cause.
    let mut governing_terminal_status: Option<u32> = None;
    let mut depth = 0usize;
    while let Some(l) = link {
        // The TS loop guards against cycles with a `seen` set; borrowed chains cannot cycle, so a
        // depth cap stands in for it.
        depth += 1;
        if depth > 64 {
            break;
        }
        if let Some(id) = l.error_id_prop() {
            kinds |= id & KIND_MASK;
        }
        if let Some(code) = l.code() {
            if account_policy_pattern().is_match(code) {
                kinds |= Flag::AccountPolicy | Flag::ContentBlocked;
            } else if anthropic_account_policy_pattern().is_match(code) {
                kinds |= Flag::AccountPolicy;
            }
        }

        match l.class() {
            ErrorClass::AwsCredentials => kinds |= Flag::AuthFailed,
            ErrorClass::AnthropicConnectionTimeout => kinds |= Flag::Timeout | Flag::Transient,
            ErrorClass::AnthropicConnection => kinds |= Flag::Transient,
            ErrorClass::CodexWebSocketTransport => kinds |= Flag::Transient,
            ErrorClass::CodexProviderStream { retryable } => {
                if retryable {
                    kinds |= Flag::Transient;
                }
            }
            ErrorClass::ProviderHttp { status: code_status, code } => {
                let mut link_kinds: u32 = 0;
                let message = l.message().unwrap_or("");
                if code == Some("usage_limit_reached")
                    || (code == Some("insufficient_quota") && !is_dash_scope_token_limit_text(message))
                    || (code_status == 402
                        && (is_402_billing_cap_body(Some(message))
                            || code.map_or(false, |c| !is_opaque_status_body(c) && is_402_billing_cap_body(Some(c)))))
                {
                    link_kinds |= Flag::UsageLimit;
                }
                if code == Some("overloaded_error") || code == Some("rate_limit_error") {
                    link_kinds |= Flag::Transient;
                }
                if code == Some("oauth_not_allowed_for_organization")
                    || code == Some("permission_error")
                    || (code_status == 403 && anthropic_account_policy_pattern().is_match(message))
                {
                    link_kinds |= Flag::AccountPolicy;
                }
                if (code_status == 401 || code_status == 403)
                    && !(code_status == 403 && parse_rate_limit_reason(message) == RateLimitReason::ConcurrentLimit)
                {
                    link_kinds |= Flag::AuthFailed;
                } else if code_status == 429 {
                    if (link_kinds & Flag::UsageLimit) == 0 {
                        link_kinds |= Flag::Transient;
                    }
                } else if code_status >= 500 {
                    link_kinds |= Flag::Transient;
                }
                kinds |= link_kinds;
            }
            ErrorClass::OAuthTokenRefresh | ErrorClass::Other => {}
        }

        let link_message = l.message().or_else(|| l.error_message());
        let link_status = status(l);
        let text_id = classify_text(link_message, link_status.or(governing_terminal_status), cause_token_evidence, api, None, None);
        kinds |= text_id & KIND_MASK;
        if is_terminal_client_error_status(link_status) {
            governing_terminal_status = link_status;
        }

        link = l.cause();
    }

    if kinds != 0 {
        create(&[kinds])
    } else {
        status(error).unwrap_or(0)
    }
}

/// Whether an error (or message string) classifies as an account usage/quota limit: the
/// persistent, credential-rotation-worthy kind. This is the public accessor for `Flag::UsageLimit`;
/// prefer it over re-running message regexes at call sites.
pub fn is_usage_limit(error: &dyn ErrorLike, api: Option<&str>) -> bool {
    is(Some(classify(error, api)), Flag::UsageLimit)
}

/// Whether an upstream rejection is an account-scoped policy denial worth retrying with a sibling credential.
pub fn is_account_policy_error(error: &dyn ErrorLike, api: Option<&str>) -> bool {
    is(Some(classify(error, api)), Flag::AccountPolicy)
}

/// Model id from Codex's exact ChatGPT-account entitlement denial. Generic unsupported-model
/// invalid requests deliberately do not match.
fn codex_chatgpt_account_policy_model(error: &dyn ErrorLike, depth: u32) -> Option<String> {
    if depth > 6 {
        return None;
    }
    let direct = error
        .error_message()
        .and_then(codex_chatgpt_account_policy_model_from_text)
        .or_else(|| error.message().and_then(codex_chatgpt_account_policy_model_from_text));
    if direct.is_some() {
        return direct;
    }
    error.cause().and_then(|c| codex_chatgpt_account_policy_model(c, depth + 1))
}

/// Whether the exact Codex entitlement denial applies to this provider and requested model.
fn is_codex_chatgpt_account_policy_error(error: &dyn ErrorLike, provider: &str, model_id: Option<&str>) -> bool {
    let denied = codex_chatgpt_account_policy_model(error, 0);
    let denied_identity = normalize_codex_chatgpt_account_policy_model(denied.as_deref());
    let requested_identity = normalize_codex_chatgpt_account_policy_model(model_id);
    provider == "openai-codex" && denied_identity.is_some() && denied_identity == requested_identity
}

/// Model id from Anthropic's HTTP 404 model-not-found envelope, not a thread or endpoint failure.
fn anthropic_model_not_found_id(error: &dyn ErrorLike, depth: u32) -> Option<String> {
    if depth > 6 {
        return None;
    }
    let message = error.error_message().or_else(|| error.message());
    if status(error) == Some(404) {
        if let Some(message) = message {
            if let Some(body) = parse_json_body(message) {
                if body.get("type").and_then(|v| v.as_str()) == Some("error") {
                    if let Some(detail) = body.get("error").and_then(|v| v.as_object()) {
                        if detail.get("type").and_then(|v| v.as_str()) == Some("not_found_error") {
                            if let Some(m) = detail.get("message").and_then(|v| v.as_str()) {
                                if let Some(rest) = m.strip_prefix("model: ") {
                                    return Some(rest.to_owned());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    error.cause().and_then(|c| anthropic_model_not_found_id(c, depth + 1))
}

/// Whether Anthropic's exact model-not-found envelope names this request's model.
fn is_anthropic_model_not_found_error(error: &dyn ErrorLike, provider: &str, model_id: Option<&str>) -> bool {
    provider == "anthropic"
        && model_id.map_or(false, |m| !m.is_empty())
        && anthropic_model_not_found_id(error, 0).as_deref() == model_id
}

/// Whether Cursor returned a non-retryable plan entitlement denial for this account.
fn is_cursor_plan_account_policy_error(error: &dyn ErrorLike, provider: &str, depth: u32) -> bool {
    if provider != "cursor" || depth > 6 {
        return false;
    }
    if error.error_message().map_or(false, is_cursor_plan_policy_text) {
        return true;
    }
    if error.message().map_or(false, is_cursor_plan_policy_text) {
        return true;
    }
    error.cause().map_or(false, |c| is_cursor_plan_account_policy_error(c, provider, depth + 1))
}

/// Flags for a rejection that denies this credential only the requested model (Codex
/// ChatGPT-account entitlement, Cursor plan, Anthropic model-not-found 404), or 0. A denial that
/// names a model must match `provider` and the requested `model_id`, so provider-controlled text
/// cannot block another model.
pub fn model_entitlement_denial_kinds(error: &dyn ErrorLike, provider: &str, model_id: Option<&str>) -> u32 {
    if is_codex_chatgpt_account_policy_error(error, provider, model_id)
        || is_cursor_plan_account_policy_error(error, provider, 0)
    {
        return Flag::AccountPolicy | Flag::ContentBlocked;
    }
    if is_anthropic_model_not_found_error(error, provider, model_id) {
        Flag::AccountPolicy
    } else {
        0
    }
}

/// Whether a rejection denies this credential only the requested model; see [`model_entitlement_denial_kinds`].
pub fn is_model_entitlement_denial(error: &dyn ErrorLike, provider: &str, model_id: Option<&str>) -> bool {
    model_entitlement_denial_kinds(error, provider, model_id) != 0
}

/// Whether a model-entitlement denial names a model other than this provider's requested one. Such
/// provider-controlled input must not change credential state.
pub fn is_mismatched_model_entitlement_denial(error: &dyn ErrorLike, provider: &str, model_id: Option<&str>) -> bool {
    let names_model =
        codex_chatgpt_account_policy_model(error, 0).is_some() || anthropic_model_not_found_id(error, 0).is_some();
    names_model && !is_model_entitlement_denial(error, provider, model_id)
}

/// Strict-tool rejection: grammar too large, schema too complex, or structured outputs unsupported
/// by the model/endpoint. Accessor for `Flag::Grammar`.
pub fn is_grammar_error(error: &dyn ErrorLike) -> bool {
    is(Some(classify(error, None)), Flag::Grammar)
}

/// Anthropic model/account does not support fast mode / the `speed` parameter. Accessor for
/// `Flag::FastModeUnsupported`.
pub fn is_fast_mode_unsupported(error: &dyn ErrorLike) -> bool {
    is(Some(classify(error, None)), Flag::FastModeUnsupported)
}

/// An Anthropic-compatible endpoint returned a 400 rejecting something about `cache_control`.
/// Whether it refused the field or one nested option is not decidable here; the caller's retry
/// ladder settles it by replaying. Accessor for `Flag::CacheControlUnsupported`.
///
/// Unlike the high-bit accessors this also requires a classified id. The flag sits in the
/// raw-status lane, and while no HTTP status reaches 0x800, `is` is a bare bit test: the extra gate
/// keeps a stray non-HTTP status out of the retry path.
pub fn is_cache_control_unsupported(error: &dyn ErrorLike) -> bool {
    let id = Some(classify(error, None));
    is_classified(id) && is(id, Flag::CacheControlUnsupported)
}

lazy_re!(cline_pass_surface_gate_pattern, r"(?i)only available via cline product surfaces");

/// Cline's gateway gates some roster entries (certain free-tier models) to its own product
/// surfaces with a 403. The API key is valid: the restriction is per-model client policy, so it
/// must neither rotate sibling credentials (they fail identically) nor surface as an auth failure.
pub fn is_cline_pass_surface_gate_message(error_message: Option<&str>) -> bool {
    error_message.map_or(false, |m| cline_pass_surface_gate_pattern().is_match(m))
}

lazy_re!(github_copilot_policy_denial_pattern, r"GitHub Copilot access denied \(HTTP 403\)");

/// GitHub Copilot 403s are plan/model-policy/org denials against a valid token, never a revoked
/// credential (those arrive as 401). Wiping stored credentials on them hides the whole provider
/// from `/model` and forces a pointless re-login, so credential-lifetime decisions must exempt them
/// even though they still classify as `Flag::AuthFailed` (issue #11275).
pub fn is_github_copilot_policy_denial(
    provider: Option<&str>,
    status: Option<u32>,
    error_message: Option<&str>,
) -> bool {
    if provider != Some("github-copilot") {
        return false;
    }
    if status == Some(403) {
        return true;
    }
    error_message.map_or(false, |m| github_copilot_policy_denial_pattern().is_match(m))
}

/// The `AssistantMessage` fields `classify_message` reads and updates.
#[derive(Debug, Clone, Default)]
pub struct ClassifiableMessage<'a> {
    pub api: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub error_id: Option<u32>,
    pub error_message: Option<&'a str>,
    pub error_classification_message: Option<&'a str>,
    pub error_status: Option<u32>,
}

/// Classify an `AssistantMessage`'s error fields. Returns the id the TS function assigns to
/// `message.errorId`; callers store it.
pub fn classify_message(message: &ClassifiableMessage<'_>) -> u32 {
    let existing_id = message.error_id;
    let current_status = message.error_status.or_else(|| status_from_id(existing_id));
    let existing_overflow_only =
        existing_id.is_some() && is(existing_id, Flag::ContextOverflow) && !is(existing_id, Flag::PayloadRejected);
    let classification_message = message.error_classification_message.or(message.error_message);
    let text_id = classify_text(
        classification_message,
        current_status,
        existing_overflow_only,
        message.api,
        message.provider,
        message.model,
    );

    let mut kinds = (existing_id.unwrap_or(0) | text_id) & KIND_MASK;
    // Two-phase finalization: drop stale status-inferred payload bit when final text proves token overflow (#9235).
    if current_status == Some(413)
        && classification_message.map_or(false, |m| !m.is_empty() && has_token_context_overflow_evidence(m))
        && (text_id & Flag::PayloadRejected) == 0
    {
        kinds &= !Flag::PayloadRejected;
    }
    if classification_message.map_or(false, |m| llama_cpp_tool_call_parse_pattern().is_match(m)) {
        // Deterministic local-model tool-call JSON parse failure: HTTP 500 is misleading because
        // the same prompt reproduces the same malformed output, so the agent-level auto-retry
        // would loop. Strip Transient so the recovery message surfaces immediately.
        kinds &= !Flag::Transient;
    }
    if kinds != 0 {
        create(&[kinds])
    } else {
        status_from_id(Some(text_id))
            .or_else(|| status_from_id(existing_id))
            .or(current_status)
            .unwrap_or(0)
    }
}

/// Overflow-classification evidence, including errors received before token usage is available.
#[derive(Debug, Clone, Default)]
pub struct ContextOverflowMessage<'a> {
    pub error_id: Option<u32>,
    pub error_status: Option<u32>,
    pub stop_reason_is_error: bool,
    pub error_message: Option<&'a str>,
    pub usage: Option<ContextOverflowUsage>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ContextOverflowUsage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub context_tokens: Option<u64>,
}

/// Provider-reported usage proves context-window excess: authoritative, compaction-owned (#9235).
///
/// Prefers `context_tokens` when the provider reports it: providers that run several model calls
/// per turn (Cursor's server-side tool loop) report `input`/`cache_read` summed across those calls,
/// which can exceed the window many times over while the conversation itself stays small.
pub fn is_usage_backed_context_overflow(message: &ContextOverflowMessage<'_>, context_window: Option<u64>) -> bool {
    let (Some(window), Some(usage)) = (context_window.filter(|w| *w != 0), message.usage) else {
        return false;
    };
    let input_tokens = usage.context_tokens.unwrap_or(usage.input + usage.cache_read + usage.cache_write);
    input_tokens > window
}

/// Classify overflow from error flags, available token usage, or provider error text.
pub fn is_context_overflow(message: &ContextOverflowMessage<'_>, context_window: Option<u64>) -> bool {
    if is(message.error_id, Flag::ContextOverflow) {
        return true;
    }
    if is_usage_backed_context_overflow(message, context_window) {
        return true;
    }
    let Some(text) = message.error_message.filter(|m| !m.is_empty()) else { return false };
    if !message.stop_reason_is_error || !matches_overflow_text(text) {
        return false;
    }
    // A rate-limit status (field, raw-status id, or the text's own "429") vetoes the text arm, as
    // in `classify_text`: "tokens" wording on a 429/402 is a throughput cap.
    let error_status = message
        .error_status
        .or_else(|| status_from_id(message.error_id))
        .or_else(|| status(&TextError(text)));
    !is_usage_limit_status(error_status)
}

/// HTTP 413 byte/media rejection (#9235); may co-occur with [`is_context_overflow`] for bare
/// `413 (no body)`. Callers with local headroom should skip compaction when this returns true.
pub fn is_payload_rejection(message: &ContextOverflowMessage<'_>) -> bool {
    if is(message.error_id, Flag::PayloadRejected) {
        return true;
    }
    let Some(text) = message.error_message.filter(|m| !m.is_empty()) else { return false };
    if !message.stop_reason_is_error {
        return false;
    }
    matches_payload_rejection_text(text)
}

/// Dual-flagged 413 (PayloadRejected + ContextOverflow) with no provider-reported token excess
/// (#9235). The co-flag means a different provider's larger byte/media budget may accept the
/// request. Usage-backed overflows are authoritative window excesses and never ambiguous.
pub fn is_text_ambiguous_context_overflow(
    error_id: u32,
    message: Option<&ContextOverflowMessage<'_>>,
    context_window: Option<u64>,
) -> bool {
    let overflow_flagged = is(Some(error_id), Flag::ContextOverflow)
        || message.map_or(false, |m| is_context_overflow(m, context_window));
    if !overflow_flagged {
        return false;
    }
    if !is(Some(error_id), Flag::PayloadRejected) {
        return false;
    }
    !message.map_or(false, |m| is_usage_backed_context_overflow(m, context_window))
}

pub fn stringify(id: Option<u32>) -> String {
    let Some(id) = id.filter(|v| *v != 0) else { return "none".to_owned() };
    if !is_classified(Some(id)) {
        return format!("status:{id}");
    }
    let labels: Vec<&str> = ERROR_KIND_LABELS
        .iter()
        .filter(|(kind, _)| is(Some(id), *kind))
        .map(|(_, label)| *label)
        .collect();
    if labels.is_empty() {
        format!("classified:0x{id:x}")
    } else {
        labels.join("|")
    }
}

lazy_re!(
    stream_parse_truncation_pattern,
    r"(?i)unterminated string|unexpected end of json input|unexpected end of data|unexpected eof|end of file|eof while parsing|truncated"
);
lazy_re!(
    stream_parse_diagnostic_pattern,
    r"(?i)(?:json parse error:\s*(?:unterminated string|unexpected end of json input|unexpected end of data|unexpected eof|end of file|eof while parsing|truncated)|json\.parse:\s*(?:unterminated string|unexpected end of data)|unexpected end of json input|unexpected eof|eof while parsing)"
);
lazy_re!(stream_event_order_pattern, r"(?i)stream event order|before message_start");

/// Transient stream corruption where the response was truncated mid-JSON.
///
/// Strings (persisted `stopDetails.explanation`/`errorMessage` diagnostics) are matched with the
/// stricter diagnostic pattern: bare "truncated"/"end of file" text is too low-signal to trust once
/// detached from a live transport `Error`, which keeps the broad pattern.
pub fn is_transient_stream_parse_error(error: &dyn ErrorLike) -> bool {
    match error.shape() {
        ErrorShape::String => error.message().map_or(false, |m| stream_parse_diagnostic_pattern().is_match(m)),
        ErrorShape::Error => error.message().map_or(false, |m| stream_parse_truncation_pattern().is_match(m)),
        ErrorShape::Object => false,
    }
}

/// Transport-level stream drops: the connection or upstream stream ended before a terminal event,
/// with no JSON-parse signal and no retryable status attached.
///
/// Distinct from the parse-truncation pattern (mid-body JSON truncation): these name the transport
/// itself dropping (proxy/gateway closing the SSE stream, socket dying before the TLS handshake
/// completes). The wording is the statusless twin of a `408 stream disconnected`, which the status
/// path already retries; an identical replay recovers it, so callers under a non-terminal status
/// treat it as transient (#11805).
lazy_re!(
    stream_drop_pattern,
    r"(?i)stream disconnected before completion|(?:stream|websocket) closed before response\.completed|stream was interrupted|stream ended before (?:terminal (?:chunk|completion event)|message_(?:start|delta|stop))|socket disconnected before secure tls connection"
);

/// Transport stream-drop diagnostic. Unlike [`is_transient_stream_parse_error`], one pattern serves
/// both the live `Error` and the persisted-string forms: the phrasings are high-signal enough to
/// trust detached from a transport `Error`.
pub fn is_transient_stream_drop_error(error: &dyn ErrorLike) -> bool {
    match error.shape() {
        ErrorShape::Object => false,
        _ => error.message().map_or(false, |m| stream_drop_pattern().is_match(m)),
    }
}

/// Any malformed stream-envelope error (prefix-tagged or out-of-order events).
pub fn is_stream_envelope_error(error: &dyn ErrorLike) -> bool {
    error.shape() == ErrorShape::Error
        && error.message().map_or(false, |m| {
            m.contains(STREAM_ENVELOPE_ERROR_PREFIX) || stream_event_order_pattern().is_match(m)
        })
}

/// Stream-envelope errors safe to retry against the provider (event ordering only).
pub fn is_retryable_stream_envelope_error(error: &dyn ErrorLike) -> bool {
    error.shape() == ErrorShape::Error && error.message().map_or(false, |m| stream_event_order_pattern().is_match(m))
}

// ---------------------------------------------------------------------------
// `ErrorLike` for the pi-ai error types
// ---------------------------------------------------------------------------

macro_rules! error_like_simple {
    ($ty:ty, $class:expr) => {
        impl ErrorLike for $ty {
            fn error_id_prop(&self) -> Option<u32> {
                HasErrorId::error_id(self)
            }
            fn message(&self) -> Option<&str> {
                Some(&self.message)
            }
            fn class(&self) -> ErrorClass<'_> {
                $class
            }
        }
    };
}

impl ErrorLike for super::abort::AbortError {
    fn error_id_prop(&self) -> Option<u32> {
        HasErrorId::error_id(self)
    }
    fn message(&self) -> Option<&str> {
        Some(&self.message)
    }
    fn name(&self) -> Option<&str> {
        Some("AbortError")
    }
}
error_like_simple!(super::auth::MissingApiKeyError, ErrorClass::Other);
error_like_simple!(super::auth::LoginCancelledError, ErrorClass::Other);
error_like_simple!(super::aws::AwsCredentialsError, ErrorClass::AwsCredentials);
error_like_simple!(super::classes::AnthropicStreamEnvelopeError, ErrorClass::Other);
error_like_simple!(super::classes::CodexWebSocketTransportError, ErrorClass::CodexWebSocketTransport);

impl HasErrorId for super::aws::AwsCredentialsError {
    fn error_id(&self) -> Option<u32> {
        None
    }
}
impl HasErrorId for super::classes::AnthropicStreamEnvelopeError {
    fn error_id(&self) -> Option<u32> {
        None
    }
}
impl HasErrorId for super::classes::CodexWebSocketTransportError {
    fn error_id(&self) -> Option<u32> {
        None
    }
}

impl ErrorLike for super::classes::AnthropicConnectionTimeoutError {
    fn message(&self) -> Option<&str> {
        Some("Request timed out.")
    }
    fn class(&self) -> ErrorClass<'_> {
        ErrorClass::AnthropicConnectionTimeout
    }
}

impl ErrorLike for super::classes::ProviderHttpError {
    fn headers(&self) -> Option<&reqwest::header::HeaderMap> {
        self.headers.as_ref()
    }
    fn error_id_prop(&self) -> Option<u32> {
        self.error_id
    }
    fn message(&self) -> Option<&str> {
        Some(&self.message)
    }
    fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }
    fn own_status(&self) -> Option<u32> {
        Some(self.status)
    }
    fn class(&self) -> ErrorClass<'_> {
        ErrorClass::ProviderHttp { status: self.status, code: self.code.as_deref() }
    }
}

impl ErrorLike for super::classes::CodexProviderStreamError {
    fn message(&self) -> Option<&str> {
        Some(&self.message)
    }
    fn class(&self) -> ErrorClass<'_> {
        ErrorClass::CodexProviderStream { retryable: self.retryable }
    }
}

impl ErrorLike for super::oauth::OAuthError {
    fn error_id_prop(&self) -> Option<u32> {
        HasErrorId::error_id(self)
    }
    fn message(&self) -> Option<&str> {
        Some(&self.message)
    }
    fn own_status(&self) -> Option<u32> {
        self.status
    }
    fn class(&self) -> ErrorClass<'_> {
        if self.kind == super::oauth::OAuthErrorKind::TokenRefresh {
            ErrorClass::OAuthTokenRefresh
        } else {
            ErrorClass::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_sets_class_bit_and_retriable_follows_kinds() {
        let id = create(&[Flag::Transient, Flag::Timeout]);
        assert!(is(Some(id), Flag::Class) && is(Some(id), Flag::Timeout));
        assert!(retriable(Some(id), None));
        assert!(!retriable(Some(id), Some(RetriableOptions { replay_unsafe: true })));
        assert!(!retriable(Some(create(&[Flag::Transient, Flag::ContentBlocked])), None));
        assert!(retriable(Some(create(&[Flag::MalformedFunctionCall])), None));
    }

    #[test]
    fn raw_status_id_is_not_classified() {
        assert_eq!(status_from_id(Some(429)), Some(429));
        assert_eq!(status_from_id(Some(create(&[Flag::Transient]))), None);
    }

    #[test]
    fn status_is_read_from_message_text() {
        assert_eq!(status_from_message("request failed with status 503"), Some(503));
        assert_eq!(status_from_message("HTTP 429 too many"), Some(429));
        assert_eq!(status_from_message("nothing here"), None);
    }

    struct Chain<'a> {
        message: &'a str,
        status: Option<u32>,
        class: ErrorClass<'a>,
        cause: Option<Box<Chain<'a>>>,
    }

    impl<'a> Chain<'a> {
        fn new(message: &'a str) -> Self {
            Self { message, status: None, class: ErrorClass::Other, cause: None }
        }
    }

    impl ErrorLike for Chain<'_> {
        fn message(&self) -> Option<&str> {
            Some(self.message)
        }
        fn own_status(&self) -> Option<u32> {
            self.status
        }
        fn cause(&self) -> Option<&dyn ErrorLike> {
            self.cause.as_deref().map(|c| c as &dyn ErrorLike)
        }
        fn class(&self) -> ErrorClass<'_> {
            self.class
        }
    }

    #[test]
    fn classify_reads_text_into_flags() {
        let id = classify(&TextError("rate limit exceeded, retry your request"), None);
        assert!(is(Some(id), Flag::Transient));
        let id = classify(&TextError("Error: prompt is too long: 250000 tokens"), None);
        assert!(is(Some(id), Flag::ContextOverflow));
        let id = classify(&TextError("401 unauthorized"), None);
        assert!(is(Some(id), Flag::AuthFailed));
        assert_eq!(classify(&TextError("hello"), None), 0);
    }

    #[test]
    fn a_429_is_never_context_overflow_and_a_413_is_a_payload_rejection() {
        let id = classify(&TextError("429 Too many tokens, please wait"), None);
        assert!(!is(Some(id), Flag::ContextOverflow));
        let id = classify_message(&ClassifiableMessage {
            error_status: Some(413),
            error_message: Some("Payload Too Large"),
            ..Default::default()
        });
        assert!(is(Some(id), Flag::PayloadRejected));
    }

    #[test]
    fn terminal_4xx_keeps_wrapped_transport_wording_terminal() {
        let mut outer = Chain::new("bad request");
        outer.status = Some(400);
        outer.class = ErrorClass::ProviderHttp { status: 400, code: None };
        outer.cause = Some(Box::new(Chain::new("unexpected EOF while reading")));
        let id = classify(&outer, None);
        assert!(!is(Some(id), Flag::Transient));
        // the same cause without the 400 wrapper is transient
        let plain = Chain::new("stream disconnected before completion");
        assert!(is(Some(classify(&plain, None)), Flag::Transient));
    }

    #[test]
    fn provider_http_codes_set_flags() {
        let mut e = Chain::new("slow down");
        e.class = ErrorClass::ProviderHttp { status: 429, code: Some("rate_limit_error") };
        assert!(is(Some(classify(&e, None)), Flag::Transient));
        let mut q = Chain::new("quota");
        q.class = ErrorClass::ProviderHttp { status: 429, code: Some("insufficient_quota") };
        assert!(is(Some(classify(&q, None)), Flag::UsageLimit));
    }

    #[test]
    fn usage_limit_and_policy_accessors() {
        assert!(is_usage_limit(&TextError("You have hit your ChatGPT usage limit"), None));
        assert!(is_account_policy_error(&TextError("blocked by cyber_policy"), None));
        assert!(!is_usage_limit(&TextError("Too many requests"), None));
    }

    #[test]
    fn model_specific_denials() {
        let msg = "The 'gpt-6' model is not supported when using Codex with a ChatGPT account.";
        let e = TextError(msg);
        assert!(is_model_entitlement_denial(&e, "openai-codex", Some("openai/gpt-6")));
        assert!(!is_model_entitlement_denial(&e, "openai-codex", Some("gpt-5")));
        assert!(is_mismatched_model_entitlement_denial(&e, "openai-codex", Some("gpt-5")));
        let nf = r#"HTTP 404 {"type":"error","error":{"type":"not_found_error","message":"model: claude-x"}}"#;
        assert!(is_model_entitlement_denial(&TextError(nf), "anthropic", Some("claude-x")));
        assert!(!is_model_entitlement_denial(&TextError(nf), "anthropic", Some("claude-y")));
    }

    #[test]
    fn strict_fast_and_cache_control_flags() {
        let g = classify_message(&ClassifiableMessage {
            error_status: Some(400),
            error_message: Some("invalid_request_error: compiled grammar is too large"),
            ..Default::default()
        });
        assert!(is(Some(g), Flag::Grammar));
        let text = "cache_control: Extra inputs are not permitted";
        let c = classify_message(&ClassifiableMessage {
            error_status: Some(400),
            error_message: Some(text),
            ..Default::default()
        });
        assert!(is(Some(c), Flag::CacheControlUnsupported));
        let mut e = Chain::new(text);
        e.status = Some(400);
        assert!(is_cache_control_unsupported(&e));
        let f = classify_message(&ClassifiableMessage {
            error_status: Some(429),
            error_message: Some("rate_limit_error: fast mode requires extra usage"),
            ..Default::default()
        });
        assert!(is(Some(f), Flag::FastModeUnsupported));
    }

    #[test]
    fn llama_cpp_parse_failure_is_not_transient() {
        let id = classify_message(&ClassifiableMessage {
            error_status: Some(500),
            error_message: Some("500 failed to parse tool call arguments as json"),
            ..Default::default()
        });
        assert!(!is(Some(id), Flag::Transient));
    }

    #[test]
    fn overflow_helpers_and_stringify() {
        let m = ContextOverflowMessage {
            stop_reason_is_error: true,
            error_message: Some("exceeds the context window"),
            ..Default::default()
        };
        assert!(is_context_overflow(&m, None));
        let rate = ContextOverflowMessage {
            stop_reason_is_error: true,
            error_status: Some(429),
            error_message: Some("exceeds the context window"),
            ..Default::default()
        };
        assert!(!is_context_overflow(&rate, None));
        let usage = ContextOverflowMessage {
            usage: Some(ContextOverflowUsage { input: 90, cache_read: 20, ..Default::default() }),
            ..Default::default()
        };
        assert!(is_usage_backed_context_overflow(&usage, Some(100)));
        assert!(!is_usage_backed_context_overflow(&usage, Some(200)));
        assert_eq!(stringify(None), "none");
        assert_eq!(stringify(Some(503)), "status:503");
        assert_eq!(stringify(Some(create(&[Flag::Transient, Flag::Timeout]))), "transient|timeout");
    }

    #[test]
    fn stream_envelope_and_drop_predicates() {
        assert!(is_stream_envelope_error(&Chain::new("Anthropic stream envelope error: x")));
        assert!(is_retryable_stream_envelope_error(&Chain::new("before message_start")));
        assert!(is_transient_stream_drop_error(&TextError("stream was interrupted")));
        assert!(is_transient_stream_parse_error(&Chain::new("Unexpected end of JSON input")));
        assert!(!is_transient_stream_parse_error(&TextError("end of file")));
        assert!(is_stream_envelope_truncation_text("Anthropic stream envelope error: ended before message_stop"));
    }

    #[test]
    fn oauth_expiry_and_small_predicates() {
        assert!(is_oauth_expiry("invalid_grant"));
        assert!(is_oauth_expiry("HTTP 401"));
        assert!(!is_oauth_expiry("401 rate limit"));
        assert!(is_cline_pass_surface_gate_message(Some("Only available via Cline product surfaces")));
        assert!(is_github_copilot_policy_denial(Some("github-copilot"), Some(403), None));
        assert!(is_immutable_latest_thinking_text(
            "messages.3.content.0: thinking blocks in the latest assistant message cannot be modified"
        ));
    }
}
