//! Port of `src/utils/http-inspector.ts`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::retry_after::{append_retry_after_hint, error_message_text, get_headers_from_error};
use crate::error::flags::{self, ErrorLike};

#[derive(Debug, Clone, Default, Serialize)]
pub struct RawHttpRequestDump {
    pub provider: String,
    pub api: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct CapturedHttpErrorResponse {
    pub status: u32,
    pub headers: Option<HeaderMap>,
    pub body_text: Option<String>,
    pub body_json: Option<Value>,
}

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            C.get_or_init(|| Regex::new($pat).expect("static pattern"))
        }
    };
}

// Matches any header name whose value carries a credential, so a persisted dump never leaks one.
// A substring match, not a hand-maintained allow-list: provider-specific auth headers
// (`x-goog-api-key`, `x-amz-security-token`, ...) are redacted without enumerating every
// provider's spelling; the fixed list it replaced silently leaked any auth header it did not name.
// Redacting a benign header that happens to match is harmless: dumps exist to diagnose the request
// body, not its transport headers.
lazy_re!(sensitive_header, r"(?i)key|token|secret|auth|credential|cookie");

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponseSummary {
    pub status: Option<u32>,
    pub message: String,
}

/// The JSON persisted for a rejected request: request fields stay at the top level (so existing
/// dump parsers still read `body`); the provider's error is added under `errorResponse`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Http400DumpPayload {
    #[serde(flatten)]
    pub dump: RawHttpRequestDump,
    pub error_response: ErrorResponseSummary,
}

/// Build the JSON persisted for a rejected request. Request fields stay at the top level (so
/// existing dump parsers still read `body`); the provider's error is added under `errorResponse`
/// so a failed request is diagnosable from the dump file rather than the request alone.
pub fn build_http_400_dump_payload(dump: &RawHttpRequestDump, error: &dyn ErrorLike, message: &str) -> Http400DumpPayload {
    Http400DumpPayload {
        dump: sanitize_dump(dump),
        error_response: ErrorResponseSummary { status: flags::status(error), message: message.to_owned() },
    }
}

/// HTTP statuses whose rejected request we persist for post-hoc diagnosis: the request-content
/// rejections that wedge a session. 400 (bad request) and 413 (payload too large: an oversized
/// image / snapcompact frame payload that 413s and empties the turn). Auth (401/403), not-found
/// (404), rate limits and 5xx are excluded: 429/5xx are retried, so persisting them here would
/// write one dump per attempt.
pub fn should_dump_rejected_request(error: &dyn ErrorLike) -> bool {
    matches!(flags::status(error), Some(400) | Some(413))
}

const RAW_HTTP_REQUEST_LINE: &str = "raw-http-request=";
const RAW_HTTP_REQUEST_SAVE_FAILED_LINE: &str = "raw-http-request-save-failed=";

/// Remove the local request-dump lines [`append_raw_http_request_dump_for_400`] appends, leaving
/// only the provider-facing error text. Hosts that relay provider errors (RPC `prompt_result`) must
/// not leak OMP-local file paths.
pub fn strip_raw_http_request_diagnostics(message: &str) -> String {
    let lines: Vec<&str> = message.split('\n').collect();
    let mut end = lines.len();
    while end > 0
        && (lines[end - 1].starts_with(RAW_HTTP_REQUEST_LINE) || lines[end - 1].starts_with(RAW_HTTP_REQUEST_SAVE_FAILED_LINE))
    {
        end -= 1;
    }
    if end == lines.len() { message.to_owned() } else { lines[..end].join("\n") }
}

/// Where dumps go and whether to write them at all. `pi-utils` supplies these in TS
/// (`getLogsDir()`, `isBunTestRuntime()`).
#[derive(Debug, Clone)]
pub struct DumpEnvironment {
    pub logs_dir: PathBuf,
    /// Never persist dumps under the test runner: providers exercise the 400 path.
    pub is_test_runtime: bool,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_owned();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

pub async fn append_raw_http_request_dump_for_400(
    message: &str,
    error: &dyn ErrorLike,
    dump: Option<&RawHttpRequestDump>,
    env: &DumpEnvironment,
) -> String {
    let Some(dump) = dump else { return message.to_owned() };
    if env.is_test_runtime || !should_dump_rejected_request(error) {
        return message.to_owned();
    }

    let payload = build_http_400_dump_payload(dump, error, message);
    let compact = serde_json::to_string(&payload).unwrap_or_default();
    // The TS name uses `Bun.hash` (Wyhash); the digest only has to be a stable short id.
    let digest = Sha256::digest(compact.as_bytes());
    let short = u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]));
    let file_name = format!("{}-{}.json", now_ms(), to_base36(short));
    let dump_dir = env.logs_dir.join(HTTP_DUMP_DIR_NAME);
    let file_path = dump_dir.join(&file_name);

    let write = async {
        tokio::fs::create_dir_all(&dump_dir).await?;
        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_default();
        tokio::fs::write(&file_path, format!("{pretty}\n")).await
    };
    match write.await {
        Ok(()) => {
            // Dumps are rare; bound the directory right after adding to it.
            let dir = dump_dir.clone();
            let keep = file_name.clone();
            tokio::spawn(async move {
                let retention = HttpRequestDumpRetention { keep: Some(keep), ..Default::default() };
                if let Err(err) = prune_http_request_dumps(&dir, retention).await {
                    tracing::warn!(dir = %dir.display(), %err, "Failed to prune HTTP request dumps");
                }
            });
            format!("{message}\n{RAW_HTTP_REQUEST_LINE}{}", file_path.display())
        }
        Err(write_error) => format!("{message}\n{RAW_HTTP_REQUEST_SAVE_FAILED_LINE}{write_error}"),
    }
}

const HTTP_DUMP_DIR_NAME: &str = "http-400-requests";
const HTTP_DUMP_MAX_AGE_MS: f64 = 7.0 * 24.0 * 60.0 * 60.0 * 1000.0;
const HTTP_DUMP_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct HttpRequestDumpRetention {
    /// Dump file name never deleted (the one just written and referenced by the error).
    pub keep: Option<String>,
    /// Dumps last modified longer ago than this are deleted (default 7 days).
    pub max_age_ms: Option<f64>,
    /// Oldest remaining dumps are deleted until the directory totals at most this (default 64 MiB).
    pub max_total_bytes: Option<u64>,
    pub now: Option<f64>,
}

/// Bound the rejected-request dump directory: delete `*.json` dumps older than the age limit, then
/// the oldest survivors until the total fits the size cap.
pub async fn prune_http_request_dumps(dir: &Path, retention: HttpRequestDumpRetention) -> std::io::Result<()> {
    let max_age_ms = retention.max_age_ms.unwrap_or(HTTP_DUMP_MAX_AGE_MS);
    let max_total_bytes = retention.max_total_bytes.unwrap_or(HTTP_DUMP_MAX_TOTAL_BYTES);
    let now = retention.now.unwrap_or(now_ms() as f64);
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    struct Dump {
        name: String,
        mtime_ms: f64,
        size: u64,
    }
    let mut dumps: Vec<Dump> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        match tokio::fs::metadata(entry.path()).await {
            Ok(meta) if meta.is_file() => {
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
                dumps.push(Dump { name, mtime_ms, size: meta.len() });
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    // Newest first; ties broken by name, descending.
    dumps.sort_by(|a, b| {
        b.mtime_ms
            .partial_cmp(&a.mtime_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.name.cmp(&a.name))
    });
    let mut retained_bytes: u64 = 0;
    for dump in dumps {
        if retention.keep.as_deref() == Some(dump.name.as_str()) {
            retained_bytes += dump.size;
            continue;
        }
        if now - dump.mtime_ms <= max_age_ms && retained_bytes + dump.size <= max_total_bytes {
            retained_bytes += dump.size;
            continue;
        }
        match tokio::fs::remove_file(dir.join(&dump.name)).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub async fn finalize_error_message(
    error: &dyn ErrorLike,
    raw_request_dump: Option<&RawHttpRequestDump>,
    captured_error_response: Option<&CapturedHttpErrorResponse>,
    env: &DumpEnvironment,
) -> String {
    let captured = format_captured_http_error_parts(captured_error_response);
    let base = error_message_text(error);
    let body = match &captured {
        Some(c) => merge_captured_body(&base, c, captured_error_response.map(|r| r.status)),
        None => base,
    };
    let headers = captured_error_response.and_then(|c| c.headers.as_ref()).or_else(|| get_headers_from_error(error));
    let message = append_retry_after_hint(&body, headers);
    append_raw_http_request_dump_for_400(&message, error, raw_request_dump, env).await
}

lazy_re!(status_prefix, r"^\s*\d{3}\s+");
lazy_re!(status_code_no_body, r"(?i)\bstatus code\s*\(no body\)");

/// Fold the captured provider body into the thrown message so it prints once. Works on the message
/// before any retry hint is appended: OpenAI-family errors cap the body they interpolate, so the
/// thrown message may hold only a prefix of the captured body, which the captured copy then replaces.
fn merge_captured_body(message: &str, captured: &CapturedHttpErrorText, status: Option<u32>) -> String {
    if status_code_no_body().is_match(message) {
        let status_text = status.map_or("HTTP".to_owned(), |s| s.to_string());
        return format!("{status_text} status code: {}", captured.text);
    }
    let status_prefix = status_prefix().find(message).map_or("", |m| m.as_str());
    let body = message[status_prefix.len()..].trim_end();
    let carries_truncated_body =
        !body.is_empty() && body.len() < captured.core.len() && captured.core.starts_with(body);
    if carries_truncated_body {
        return with_suffix(&format!("{status_prefix}{}", captured.core), captured);
    }
    if message.contains(&captured.core) {
        return with_suffix(message, captured);
    }
    format!("{message}\n{}", captured.text)
}

fn with_suffix(message: &str, captured: &CapturedHttpErrorText) -> String {
    match &captured.suffix {
        Some(suffix) if !suffix.is_empty() && !message.contains(suffix.as_str()) => format!("{message}{suffix}"),
        _ => message.to_owned(),
    }
}

/// Rewrite error message for GitHub Copilot request failures. Must run AFTER
/// [`finalize_error_message`] since it replaces the message entirely.
///
/// 401 = token invalid/expired: credential removal is safe, prompt re-login.
/// 403 = token valid but access denied (plan, model policy, org restriction): do NOT reuse the
///       auth-failed string (which triggers credential removal).
/// 400 is left verbatim: GitHub's body names the cause (`model_not_supported`,
///       `model_not_available_for_integrator` with its `Available models` list) and a rewrite
///       only hides it.
pub fn rewrite_copilot_error(error_message: &str, error: &dyn ErrorLike, provider: &str) -> String {
    if provider != "github-copilot" {
        return error_message.to_owned();
    }
    match flags::status(error) {
        Some(401) => "GitHub Copilot authentication failed (HTTP 401). Your token may have been revoked. Please re-login with /login github-copilot".to_owned(),
        Some(403) => "GitHub Copilot access denied (HTTP 403). Your token is valid but the account may not have access to this model or feature. Check your Copilot plan or model policy settings. Business organizations can also restrict which clients may call the API: omp sends Copilot-Integration-Id copilot-chat by default (COPILOT_INTEGRATION_ID overrides it) and retries a denied default-identity request once as the Copilot CLI (copilot-developer-cli). If both identities are denied, ask your org admin to allow one of them; if you pinned an identity, try the other.".to_owned(),
        _ => error_message.to_owned(),
    }
}

lazy_re!(
    cline_pass_not_subscribed,
    r"(?i)not subscribed to required model plan|no access to clinepass subscription models"
);
lazy_re!(
    cline_pass_org_account,
    r"(?i)organization accounts cannot use individual model inference subscriptions"
);
lazy_re!(cline_pass_model_not_found, r"(?i)model not found");

/// Rewrite error messages for ClinePass request failures. Gated to the cline-pass provider: the
/// "model not found" marker is too generic to match for other hosts.
///
/// not-subscribed (400) = the key is valid but the account has no ClinePass subscription;
///        free-tier models remain usable on the same key.
/// org restriction (400) = organization accounts cannot use individual inference subscriptions; a
///        personal-account key is required.
/// model-not-found (400) = roster rotation removed the model since selection; the fix is
///        reselection, not retry. (Quota windows are classified upstream in `error/rate_limit` and
///        need no rewrite.)
/// surface-gate (403) = the model is restricted to Cline's official clients. Requests carry the
///        mirrored CLI identity headers, so reaching this means Cline's gate policy changed; the
///        classifier exempts it from credential rotation (sibling keys fail identically).
pub fn rewrite_cline_pass_error(error_message: &str, provider: &str) -> String {
    if provider != "cline-pass" {
        return error_message.to_owned();
    }
    if cline_pass_not_subscribed().is_match(error_message) {
        return "This model requires a ClinePass subscription. Free-tier models (marked \"(free)\" in the picker) work with any Cline account.".to_owned();
    }
    if cline_pass_org_account().is_match(error_message) {
        return "ClinePass is unavailable for organization accounts: individual inference subscriptions are personal-plan only. Log in with a personal Cline API key.".to_owned();
    }
    if flags::is_cline_pass_surface_gate_message(Some(error_message)) {
        return "Cline restricts this model to its official product surfaces and the mirrored CLI client identity was not accepted. Pick another model with /model and report the regression — the header mirror may need updating.".to_owned();
    }
    if cline_pass_model_not_found().is_match(error_message) {
        return "Cline removed this model from the roster since it was selected. Pick another with /model — the roster refreshes automatically while your API key is configured.".to_owned();
    }
    error_message.to_owned()
}

fn sanitize_dump(dump: &RawHttpRequestDump) -> RawHttpRequestDump {
    RawHttpRequestDump {
        url: redact_url_query(dump.url.as_deref()),
        headers: redact_headers(dump.headers.as_ref()),
        ..dump.clone()
    }
}

/// Strips a persisted dump's query string entirely rather than picking sensitive params by name: a
/// configurable `baseUrl` (e.g. Bedrock's gateway routing) can carry an arbitrary query-based
/// credential the way `SENSITIVE_HEADER_PATTERN` matches arbitrary header names, and dumps exist
/// to diagnose the request body, not the query.
fn redact_url_query(url: Option<&str>) -> Option<String> {
    let url = url.filter(|u| !u.is_empty())?;
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            if parsed.query().map_or(true, str::is_empty) && !url.contains('?') {
                return Some(url.to_owned());
            }
            parsed.set_query(None);
            Some(format!("{parsed}[redacted-query]"))
        }
        Err(_) => Some(url.to_owned()),
    }
}

/// Copy of `headers` with every credential value replaced by `[redacted]`; shared by every
/// persisted dump.
pub fn redact_headers(headers: Option<&BTreeMap<String, String>>) -> Option<BTreeMap<String, String>> {
    let headers = headers?;
    let mut redacted = BTreeMap::new();
    for (key, value) in headers {
        if sensitive_header().is_match(key) {
            redacted.insert(key.clone(), "[redacted]".to_owned());
        } else {
            redacted.insert(key.clone(), value.clone());
        }
    }
    Some(redacted)
}

#[derive(Debug, Clone)]
struct CapturedHttpErrorText {
    /// Provider-supplied message with the envelope's structured fields removed.
    core: String,
    /// `core` followed by the `(type=... param=... code=...)` suffix when the payload carries one.
    text: String,
    /// Suffix alone, leading space included; absent when the payload has no type/param/code.
    suffix: Option<String>,
}

fn format_captured_http_error_parts(captured: Option<&CapturedHttpErrorResponse>) -> Option<CapturedHttpErrorText> {
    let captured = captured?;
    let body_text = captured.body_text.as_deref()?.trim();
    if body_text.is_empty() {
        return None;
    }
    let Some(payload) = parse_captured_error_payload(captured) else {
        return Some(CapturedHttpErrorText { core: body_text.to_owned(), text: body_text.to_owned(), suffix: None });
    };

    let error_payload = get_object_property(&payload, "error");
    let payload_is_error_payload = error_payload.is_none();
    let error_payload: &Map<String, Value> = error_payload.unwrap_or(&payload);
    // {"error": "string"}: the error value is a plain string, not a nested object. Fall back to it
    // when the structured fields ("message", etc.) are absent.
    let string_error = if payload_is_error_payload { get_string_property(&payload, "error") } else { None };
    let core = get_string_property(error_payload, "message")
        .or_else(|| get_string_property(&payload, "message"))
        .or(string_error)
        .unwrap_or_else(|| body_text.to_owned());
    let extras: Vec<String> = [
        get_string_property(error_payload, "type").or_else(|| get_string_property(&payload, "type")),
        get_string_property(error_payload, "param").or_else(|| get_string_property(&payload, "param")),
        get_string_property(error_payload, "code").or_else(|| get_string_property(&payload, "code")),
    ]
    .into_iter()
    .flatten()
    .filter(|v| !v.is_empty())
    .enumerate()
    .map(|(index, value)| match index {
        0 => format!("type={value}"),
        1 => format!("param={value}"),
        _ => format!("code={value}"),
    })
    .collect();
    if extras.is_empty() {
        return Some(CapturedHttpErrorText { text: core.clone(), core, suffix: None });
    }
    let suffix = format!(" ({})", extras.join(" "));
    Some(CapturedHttpErrorText { text: format!("{core}{suffix}"), core, suffix: Some(suffix) })
}

fn parse_captured_error_payload(captured: &CapturedHttpErrorResponse) -> Option<Map<String, Value>> {
    if let Some(Value::Object(m)) = &captured.body_json {
        return Some(m.clone());
    }
    let text = captured.body_text.as_deref().filter(|t| !t.is_empty())?;
    match serde_json::from_str::<Value>(text).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn get_object_property<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    value.get(key).and_then(Value::as_object)
}

fn get_string_property(value: &Map<String, Value>, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classes::{ProviderHttpError, ProviderHttpErrorOptions};
    use crate::error::flags::TextError;
    use serde_json::json;

    fn env() -> DumpEnvironment {
        DumpEnvironment { logs_dir: std::env::temp_dir(), is_test_runtime: true }
    }

    fn captured(status: u32, body: &str) -> CapturedHttpErrorResponse {
        CapturedHttpErrorResponse { status, body_text: Some(body.to_owned()), ..Default::default() }
    }

    #[tokio::test]
    async fn captured_body_replaces_a_truncated_prefix_and_adds_the_suffix() {
        let body = r#"{"error":{"message":"The prompt is far too long for this model","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
        let err = ProviderHttpError::new("400 The prompt is far", 400, ProviderHttpErrorOptions::default());
        let msg = finalize_error_message(&err, None, Some(&captured(400, body)), &env()).await;
        assert_eq!(
            msg,
            "400 The prompt is far too long for this model (type=invalid_request_error param=context_length_exceeded)"
        );
        // The TS labels extras by position after dropping the missing ones, so a lone `code` is
        // printed as `param=`; the port keeps that behavior.
    }

    #[tokio::test]
    async fn no_body_message_is_rebuilt_from_the_captured_text() {
        let err = ProviderHttpError::new("400 status code (no body)", 400, ProviderHttpErrorOptions::default());
        let msg = finalize_error_message(&err, None, Some(&captured(400, "plain text failure")), &env()).await;
        assert_eq!(msg, "400 status code: plain text failure");
    }

    #[tokio::test]
    async fn unrelated_captured_body_is_appended_on_a_new_line() {
        let err = ProviderHttpError::new("500 boom", 500, ProviderHttpErrorOptions::default());
        let msg = finalize_error_message(&err, None, Some(&captured(500, "different text")), &env()).await;
        assert_eq!(msg, "500 boom\ndifferent text");
    }

    #[test]
    fn redaction_hides_credentials_and_query_strings() {
        let mut h = BTreeMap::new();
        h.insert("x-goog-api-key".to_owned(), "secret".to_owned());
        h.insert("content-type".to_owned(), "json".to_owned());
        let r = redact_headers(Some(&h)).unwrap();
        assert_eq!(r["x-goog-api-key"], "[redacted]");
        assert_eq!(r["content-type"], "json");
        assert_eq!(redact_url_query(Some("https://h/x?key=1")).as_deref(), Some("https://h/x[redacted-query]"));
        assert_eq!(redact_url_query(Some("https://h/x")).as_deref(), Some("https://h/x"));
    }

    #[test]
    fn dumps_only_for_400_and_413_and_payload_shape() {
        assert!(should_dump_rejected_request(&TextError("HTTP 400 bad")));
        assert!(!should_dump_rejected_request(&TextError("HTTP 429 slow")));
        let dump = RawHttpRequestDump { provider: "p".into(), api: "a".into(), model: "m".into(), body: Some(json!({"x": 1})), ..Default::default() };
        let payload = build_http_400_dump_payload(&dump, &TextError("HTTP 400 bad"), "bad");
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["errorResponse"], json!({"status": 400, "message": "bad"}));
        assert_eq!(v["body"], json!({"x": 1}));
    }

    #[test]
    fn strip_removes_trailing_dump_lines_only() {
        assert_eq!(strip_raw_http_request_diagnostics("msg\nraw-http-request=/tmp/x.json"), "msg");
        assert_eq!(strip_raw_http_request_diagnostics("a\nb"), "a\nb");
    }

    #[test]
    fn provider_message_rewrites() {
        let e401 = TextError("HTTP 401");
        assert!(rewrite_copilot_error("x", &e401, "github-copilot").contains("(HTTP 401)"));
        assert_eq!(rewrite_copilot_error("x", &e401, "other"), "x");
        assert!(rewrite_cline_pass_error("Model not found", "cline-pass").starts_with("Cline removed this model"));
        assert_eq!(rewrite_cline_pass_error("Model not found", "other"), "Model not found");
    }

    #[tokio::test]
    async fn prune_removes_old_dumps_but_keeps_the_named_one() {
        let dir = std::env::temp_dir().join(format!("pi-ai-prune-{}", now_ms()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("old.json"), "{}").await.unwrap();
        tokio::fs::write(dir.join("keep.json"), "{}").await.unwrap();
        tokio::fs::write(dir.join("note.txt"), "x").await.unwrap();
        let far_future = now_ms() as f64 + 30.0 * 24.0 * 3600.0 * 1000.0;
        prune_http_request_dumps(
            &dir,
            HttpRequestDumpRetention { keep: Some("keep.json".into()), now: Some(far_future), ..Default::default() },
        )
        .await
        .unwrap();
        assert!(!dir.join("old.json").exists());
        assert!(dir.join("keep.json").exists());
        assert!(dir.join("note.txt").exists());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
