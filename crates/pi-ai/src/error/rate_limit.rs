//! Port of `src/error/rate-limit.ts`.
//!
//! Rate limit reason classification and backoff calculation utilities. Ported from the
//! opencode-antigravity-auth plugin for consistency.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::pi_utils::fetch_retry::extract_retry_hint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitReason {
    QuotaExhausted,
    InsufficientG1CreditsBalance,
    RateLimitExceeded,
    ConcurrentLimit,
    ModelCapacityExhausted,
    ServerError,
    Unknown,
}

impl RateLimitReason {
    /// The TS string literal.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuotaExhausted => "QUOTA_EXHAUSTED",
            Self::InsufficientG1CreditsBalance => "INSUFFICIENT_G1_CREDITS_BALANCE",
            Self::RateLimitExceeded => "RATE_LIMIT_EXCEEDED",
            Self::ConcurrentLimit => "CONCURRENT_LIMIT",
            Self::ModelCapacityExhausted => "MODEL_CAPACITY_EXHAUSTED",
            Self::ServerError => "SERVER_ERROR",
            Self::Unknown => "UNKNOWN",
        }
    }
}

const QUOTA_EXHAUSTED_BACKOFF_MS: f64 = 30.0 * 60.0 * 1000.0; // 30 min
const RATE_LIMIT_EXCEEDED_BACKOFF_MS: f64 = 30.0 * 1000.0; // 30s
const CONCURRENT_LIMIT_BACKOFF_MS: f64 = 5.0 * 1000.0; // 5s
const MODEL_CAPACITY_BASE_MS: f64 = 45.0 * 1000.0; // 45s base
const MODEL_CAPACITY_JITTER_MS: f64 = 30.0 * 1000.0; // +/-15s
const SERVER_ERROR_BACKOFF_MS: f64 = 20.0 * 1000.0; // 20s

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            C.get_or_init(|| Regex::new($pat).expect("static pattern"))
        }
    };
}

lazy_re!(
    account_rate_limit,
    r"(?i)\baccount(?:'s)?\b[^\n]{0,80}\brate.?limit\b|\brate.?limit\b[^\n]{0,80}\baccount\b"
);
lazy_re!(insufficient_balance, r"(?i)insufficient.?balance");
// Prepaid-credit exhaustion phrased around the credit balance rather than a quota: Anthropic "This
// request would exceed your available credits given your current in-flight requests" (402),
// OpenRouter "Insufficient credits", "credits exhausted". Account-local, so rotate to a sibling credential.
lazy_re!(
    credits_exhausted,
    r"(?i)\b(?:exceed\w*|insufficient|not enough)\b[^\n]{0,40}\bcredits?\b|\bcredits?\b[^\n]{0,40}\b(?:exhausted|depleted)\b"
);
// Anthropic subscription entitlement wall: "Usage credits are required for this model" with
// `error_code: credits_required`. The account cannot serve the model at all, so rotate to a sibling.
// Bounded to the documented sentence and the exact code: bare "usage credits" also appears in
// unrelated diagnostics, which must not rotate a healthy credential.
lazy_re!(anthropic_credits_required, r"(?i)\busage credits are required\b|\bcredits_required\b");
// Prepaid-balance exhaustion: Cursor ERROR_USAGE_PRICING_REQUIRED (code 44, surfaced as 429). The
// `\b` after the code keeps USAGE_PRICING_REQUIRED_CHANGEABLE out.
lazy_re!(
    prepaid_balance_exhausted,
    r"(?i)\busage_pricing_required\b|\bprepaid balance\b[^\n]{0,40}\b(?:used up|exhausted|depleted)\b"
);
// Account billing ceilings: Anthropic "monthly spend limit" (#4787) and Google "Your project has
// exceeded its monthly spending cap" (#13090). The `\b` after `cap` keeps "spending capacity" out.
lazy_re!(spend_limit, r"(?i)spend(?:ing)?[\s_-]?(?:limit|cap)\b");
lazy_re!(
    subscription_cap,
    r"(?i)\b(?:subscription|plan|membership)\b[^\n]{0,80}\b(?:rate.?limits?|quota|cap)\b|\b(?:rate.?limits?|quota|cap)\b[^\n]{0,80}\b(?:subscription|plan|membership)\b"
);
lazy_re!(transient_interval_rate_limit, r"(?i)\bper\s+(?:second|minute)\b");

fn matches_subscription_cap_text(error_message: &str) -> bool {
    subscription_cap().is_match(error_message) && !transient_interval_rate_limit().is_match(error_message)
}

lazy_re!(openrouter_daily_free_limit, r"(?i)\bfree[-_ ]models[-_ ]per[-_ ]day\b");
// ClinePass subscription-window exhaustion ("clinepass limit ...") and free-tier model caps ("free
// limit reached on model ... try again in ...") are account-local quota exhaustion.
lazy_re!(cline_pass_quota, r"(?i)clinepass limit|free limit reached on model");
// gRPC/Connect end-streams carry the status as its name (`resource_exhausted`), while HTTP bodies
// use the phrase ("resource exhausted"). Strip either form before classifying explicit details.
lazy_re!(resource_exhausted, r"(?i)resource.?exhausted");
// Require an actual cap signal near "concurrent". "Too many concurrent requests" is itself a cap
// signal; bare feature rejections such as "concurrent invocation is not supported" remain excluded.
lazy_re!(
    concurrent_limit,
    r"(?i)\btoo many\s+concurren\w*\s+(?:requests?|invocations?)\b|\bconcurren\w*\b[^\n]{0,60}\b(?:limit|quota|exceed\w*|reach\w*)\b|\b(?:limit|quota|exceed\w*|reach\w*)\b[^\n]{0,60}\bconcurren\w*\b|\bconcurren[a-z]*[-_](?:[a-z]+[_-])*(?:limit|quota|exceed\w*|reach\w*)"
);
// The bare "limit will reset" / "will reset in" phrasing also appears on statusless per-minute
// transients, so gate the reset-window alternative on account-specific wording (Devin's "Your limit
// will reset in ...").
lazy_re!(
    account_scoped_403,
    r"(?i)\b(?:overall|account|organization|team|workspace)\b[^\n]{0,40}\b(?:message |request )?rate.?limit\b|\byour\b[^\n]{0,30}\b(?:limit )?will reset\b"
);
// Simplified Chinese account-quota exhaustion phrasing (Zhipu Coding Plan, MiniMax CN, ...). The
// 上限 arm is anchored on the 使用 / 用量 tokens so a rate/concurrency cap does NOT match.
lazy_re!(
    cn_quota_exhausted,
    r"(?:使用|用量).{0,30}?上限|(?:额度|配额)已?(?:用|耗)(?:完|尽)|限额.{0,30}重置|余额不足"
);
// Simplified Chinese rate/concurrency caps can contain both 使用 and 上限, but remain transient.
lazy_re!(
    cn_transient_cap,
    r"速率.{0,30}上限|频率.{0,30}上限|每分钟.{0,30}上限|并发.{0,30}上限|使用.{0,30}(?:速率|频率|每分钟|并发).{0,30}上限"
);
// Common Simplified Chinese throttle phrasing, consulted by `is_opaque_status_body`.
lazy_re!(cn_throttle, r"速率(?:限制|过快)|频率(?:过高|过快)|过于频繁|稍后[重再]试");
// DashScope / Bailian reports its per-minute token throttle with OpenAI-compatible billing wording
// and links the error-code doc's `token-limit` anchor: a transient TPM/TPS cap, not account quota.
lazy_re!(dashscope_token_limit_doc, r"(?i)error-code[^()\s]*#token-limit");
lazy_re!(
    dashscope_token_limit_message,
    r"(?i)\byou exceeded your current quota, please check your plan and billing details\b"
);

/// True for DashScope/Bailian's documented OpenAI-compatible TPM/TPS throttle.
pub fn is_dash_scope_token_limit_text(error_message: &str) -> bool {
    dashscope_token_limit_doc().is_match(error_message) && dashscope_token_limit_message().is_match(error_message)
}

// Rolling per-minute token/request throttles (TPM/RPM): the window self-heals within the minute, so
// they belong in the transient backoff lane, not the 30-minute credential-blocking quota lane (#13253).
lazy_re!(
    tpm_rpm_throttle,
    r"(?i)\b(?:tpm|rpm)\b[^\n]{0,40}\b(?:exhaust\w*|exceed\w*|limit\w*|throttl\w*|reach\w*)\b|\b(?:exhaust\w*|exceed\w*|limit\w*|throttl\w*|reach\w*)\b[^\n]{0,40}\b(?:tpm|rpm)\b|\bRateLimitExceeded\.(?:Endpoint)?(?:TPM|RPM)\w*"
);

const GOOGLE_RPC_ERROR_INFO_TYPE: &str = "type.googleapis.com/google.rpc.ErrorInfo";
const GOOGLE_RPC_QUOTA_FAILURE_TYPE: &str = "type.googleapis.com/google.rpc.QuotaFailure";
const GOOGLE_RPC_RETRY_INFO_TYPE: &str = "type.googleapis.com/google.rpc.RetryInfo";
lazy_re!(antigravity_model_quota, r"(?i)\bexhausted your capacity on this model\b");
const LONG_RATE_LIMIT_DELAY_MS: f64 = 5.0 * 60.0 * 1000.0;

fn as_record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// Parse an embedded JSON object from an error message.
pub fn parse_json_body(error_message: &str) -> Option<Map<String, Value>> {
    let start = error_message.find('{')?;
    let end = error_message.rfind('}')?;
    if end < start {
        return None;
    }
    let parsed: Value = serde_json::from_str(&error_message[start..=end]).ok()?;
    match parsed {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// Classify structured Google RESOURCE_EXHAUSTED bodies before consulting text. Cloud Code Assist
/// prefixes the JSON with its HTTP error label, so accept an embedded top-level object as well as a
/// raw JSON body.
fn parse_google_rpc_rate_limit_reason(error_message: &str) -> Option<RateLimitReason> {
    let body = parse_json_body(error_message)?;
    let error = as_record(body.get("error"))?;
    let status = error.get("status").and_then(Value::as_str)?;
    if status.trim().to_uppercase() != "RESOURCE_EXHAUSTED" {
        return None;
    }
    let details = error.get("details").and_then(Value::as_array)?;

    // 1. ErrorInfo details take first precedence for terminal quota/capacity reasons, while a
    // generic RATE_LIMIT_EXCEEDED is held until QuotaFailure is checked so a co-reported daily cap
    // or zero-entitlement violation still rotates credentials.
    let mut saw_generic_rate_limit_info = false;
    for value in details {
        let Some(detail) = as_record(Some(value)) else { continue };
        if detail.get("@type").and_then(Value::as_str) != Some(GOOGLE_RPC_ERROR_INFO_TYPE) {
            continue;
        }
        let Some(reason) = detail.get("reason").and_then(Value::as_str) else { continue };
        match reason.trim().to_uppercase().as_str() {
            "MODEL_CAPACITY_EXHAUSTED" | "MODEL_CAPACITY_EXCEEDED" => return Some(RateLimitReason::ModelCapacityExhausted),
            "QUOTA_EXHAUSTED" => return Some(RateLimitReason::QuotaExhausted),
            // Keep Google's specific credit-balance reason available to logs and callers while
            // treating it as credential-rotatable below.
            "INSUFFICIENT_G1_CREDITS_BALANCE" => return Some(RateLimitReason::InsufficientG1CreditsBalance),
            "RATE_LIMIT_EXCEEDED" => {
                // Cloud Code Assist also uses this reason for an account's per-model quota, even
                // when that quota resets within seconds.
                let in_message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or(false, |m| antigravity_model_quota().is_match(m));
                if in_message || antigravity_model_quota().is_match(error_message) {
                    return Some(RateLimitReason::QuotaExhausted);
                }
                saw_generic_rate_limit_info = true;
            }
            _ => {}
        }
    }

    // 2. QuotaFailure details take second precedence. Inspect all violations before returning a
    // short-window transient verdict so any daily cap or zero-entitlement quotaValue ("0") dominates.
    let mut saw_short_window_violation = false;
    for value in details {
        let Some(detail) = as_record(Some(value)) else { continue };
        if detail.get("@type").and_then(Value::as_str) != Some(GOOGLE_RPC_QUOTA_FAILURE_TYPE) {
            continue;
        }
        let Some(violations) = detail.get("violations").and_then(Value::as_array) else { continue };
        for v in violations {
            let violation = as_record(Some(v));
            let quota_value = violation.and_then(|v| v.get("quotaValue"));
            let quota_value_text = match quota_value {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            };
            if let Some(text) = quota_value_text {
                static ZERO: OnceLock<Regex> = OnceLock::new();
                let zero = ZERO.get_or_init(|| Regex::new(r"^\s*0+(?:\.0+)?\s*$").unwrap());
                if zero.is_match(&text) {
                    return Some(RateLimitReason::QuotaExhausted);
                }
            }
            let quota_id = violation.and_then(|v| v.get("quotaId")).and_then(Value::as_str).unwrap_or("").to_lowercase();
            let quota_metric = violation.and_then(|v| v.get("quotaMetric")).and_then(Value::as_str).unwrap_or("").to_lowercase();
            let combined = format!("{quota_id} {quota_metric}");
            if combined.contains("perday") || combined.contains("daily") || combined.contains("per_day") {
                return Some(RateLimitReason::QuotaExhausted);
            }
            if combined.contains("perminute")
                || combined.contains("per_minute")
                || combined.contains("persecond")
                || combined.contains("per_second")
                || combined.contains("per100seconds")
                || combined.contains("per_100_seconds")
            {
                saw_short_window_violation = true;
            }
        }
    }
    if saw_short_window_violation || saw_generic_rate_limit_info {
        let retry_delay_ms = extract_retry_hint(None, Some(error_message), None);
        return Some(match retry_delay_ms {
            Some(ms) if ms >= LONG_RATE_LIMIT_DELAY_MS => RateLimitReason::QuotaExhausted,
            _ => RateLimitReason::RateLimitExceeded,
        });
    }

    // 3. Standalone RetryInfo when neither ErrorInfo nor QuotaFailure returned a verdict
    for value in details {
        let Some(detail) = as_record(Some(value)) else { continue };
        if detail.get("@type").and_then(Value::as_str) != Some(GOOGLE_RPC_RETRY_INFO_TYPE) {
            continue;
        }
        if let Some(ms) = extract_retry_hint(None, Some(error_message), None) {
            return Some(if ms >= LONG_RATE_LIMIT_DELAY_MS {
                RateLimitReason::QuotaExhausted
            } else {
                RateLimitReason::RateLimitExceeded
            });
        }
    }

    None
}

fn is_quota_exhausted_reason(reason: RateLimitReason) -> bool {
    matches!(reason, RateLimitReason::QuotaExhausted | RateLimitReason::InsufficientG1CreditsBalance)
}

/// Classify a rate-limit error message into a reason category.
/// Priority order: explicit details in a resource-exhausted error > QUOTA (Antigravity "quota will
/// reset") > CN quota > DASHSCOPE_TOKEN_LIMIT (TPM/TPS throttle) > CONCURRENT_LIMIT > MODEL_CAPACITY
/// > QUOTA (account) > RATE_LIMIT (including TPM/RPM rolling windows) > QUOTA (generic) >
/// SERVER_ERROR > bare resource-exhausted > UNKNOWN.
///
/// Bare "resource exhausted" / "resource_exhausted" maps to MODEL_CAPACITY (transient, short wait).
/// Explicit details such as "quota exceeded" retain their normal classification.
pub fn parse_rate_limit_reason(error_message: &str) -> RateLimitReason {
    use RateLimitReason as R;
    if let Some(structured) = parse_google_rpc_rate_limit_reason(error_message) {
        return structured;
    }
    let lower_with_status = error_message.to_lowercase();
    let lower = resource_exhausted().replace_all(&lower_with_status, "").into_owned();
    let has_resource_exhausted_status = lower != lower_with_status;

    // Antigravity / Cloud Code Assist surface multi-hour daily-quota exhaustion as "You have
    // exhausted your capacity on this model. Your quota will reset after ...". The literal
    // "capacity" used to pre-empt the QUOTA branch, so short-circuit here before the MODEL_CAPACITY
    // fallthrough so credential rotation (not 60s backoff) kicks in.
    if lower.contains("quota will reset") || lower.contains("exhausted your capacity") {
        return R::QuotaExhausted;
    }

    // Simplified Chinese quota-exhaustion phrasing. Must precede the MODEL_CAPACITY / RATE_LIMIT
    // branches so an account-local cap rotates instead of backing off as a transient.
    if cn_quota_exhausted().is_match(error_message) && !cn_transient_cap().is_match(error_message) {
        return R::QuotaExhausted;
    }

    // DashScope/Bailian TPM/TPS throttle: billing-worded like OpenAI's account quota, but the doc
    // anchor marks it a per-minute token cap that clears on its own.
    if is_dash_scope_token_limit_text(error_message) {
        return R::RateLimitExceeded;
    }

    if concurrent_limit().is_match(error_message) {
        return R::ConcurrentLimit;
    }

    if lower.contains("capacity") || lower.contains("overloaded") || lower.contains("529") || lower.contains("503") {
        return R::ModelCapacityExhausted;
    }

    if account_rate_limit().is_match(error_message) {
        return R::QuotaExhausted;
    }
    if spend_limit().is_match(error_message) {
        return R::QuotaExhausted;
    }
    if matches_subscription_cap_text(error_message) {
        return R::QuotaExhausted;
    }
    if openrouter_daily_free_limit().is_match(error_message) {
        return R::QuotaExhausted;
    }
    if cline_pass_quota().is_match(error_message) {
        return R::QuotaExhausted;
    }
    if anthropic_credits_required().is_match(error_message) {
        return R::QuotaExhausted;
    }
    if prepaid_balance_exhausted().is_match(error_message) {
        return R::QuotaExhausted;
    }

    if lower.contains("per minute")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("presque")
        || tpm_rpm_throttle().is_match(error_message)
    {
        return R::RateLimitExceeded;
    }

    if lower.contains("exhausted")
        || lower.contains("quota")
        || lower.contains("usage limit")
        // xAI SuperGrok: HTTP 403 "run out of credits" / spending-limit is an account-local cap:
        // rotate, don't treat as auth failure.
        || lower.contains("run out of credits")
        || lower.contains("out of credits")
        || lower.contains("spending-limit")
        || lower.contains("spending limit")
        || lower.contains("access_terminated_error")
        || insufficient_balance().is_match(error_message)
        || credits_exhausted().is_match(error_message)
    {
        return R::QuotaExhausted;
    }

    if lower.contains("500") || lower.contains("internal error") || lower.contains("internal server error") {
        return R::ServerError;
    }

    if has_resource_exhausted_status {
        return R::ModelCapacityExhausted;
    }

    R::Unknown
}

/// Calculate backoff delay in ms for a given rate limit reason. MODEL_CAPACITY gets jitter to
/// prevent thundering herd. `jitter_unit` is the `Math.random()` draw in `[0, 1)`.
pub fn calculate_rate_limit_backoff_ms(reason: RateLimitReason, jitter_unit: f64) -> f64 {
    match reason {
        RateLimitReason::InsufficientG1CreditsBalance | RateLimitReason::QuotaExhausted => QUOTA_EXHAUSTED_BACKOFF_MS,
        RateLimitReason::RateLimitExceeded => RATE_LIMIT_EXCEEDED_BACKOFF_MS,
        RateLimitReason::ConcurrentLimit => CONCURRENT_LIMIT_BACKOFF_MS,
        RateLimitReason::ModelCapacityExhausted => MODEL_CAPACITY_BASE_MS + jitter_unit * MODEL_CAPACITY_JITTER_MS,
        RateLimitReason::ServerError => SERVER_ERROR_BACKOFF_MS,
        // conservative default
        RateLimitReason::Unknown => QUOTA_EXHAUSTED_BACKOFF_MS,
    }
}

/// Detect usage/quota limit errors in error messages (persistent, requires credential switch).
lazy_re!(
    usage_limit,
    r"(?i)usage.?limit|usage_limit_reached|usage_not_included|limit_reached|quota.?(?:exceeded|reached|insufficient)|额度不足|额度耗尽|resource.?exhausted|exhausted your capacity|quota will reset|insufficient.?(?:balance|quota)|balance.?exhausted|run out of credits|out of credits|out of (?:extra )?usage|spending[- _]?limit|personal-team-blocked|clinepass limit|free limit reached on model|access_terminated_error"
);

/// HTTP status codes that, absent richer body classification, represent an account-local usage cap
/// rather than a bad credential or a transient blip. HTTP 402 Payment Required represents an
/// account-billing cap (xAI Grok Build "usage balance exhausted", DeepSeek "Insufficient Balance",
/// OpenCode Go "Insufficient account funds", OpenRouter credit exhaustion) when opaque,
/// payment/deactivation/balance/funds-worded, or QUOTA_EXHAUSTED/CONCURRENT_LIMIT. Informative
/// non-quota 402s (e.g. endpoint subscription requirements) remain non-usage-limits. Always combine
/// with [`is_usage_limit_outcome`] when a message is available.
pub fn is_usage_limit_status(status: Option<u32>) -> bool {
    status == Some(429) || status == Some(402)
}

lazy_re!(
    status_402_quota,
    r"(?i)\b(?:payment(?:\s+is)?[-_.\s]*required|deactivated_workspace|insufficient.?(?:balance|account.?funds))\b"
);

pub fn is_402_billing_cap_body(message: Option<&str>) -> bool {
    let Some(message) = message else { return true };
    if is_opaque_status_body(message) {
        return true;
    }
    if status_402_quota().is_match(message) {
        return true;
    }
    let reason = parse_rate_limit_reason(message);
    is_quota_exhausted_reason(reason) || reason == RateLimitReason::ConcurrentLimit
}

// Billing-cap wording: the account is out of money the user must add, as opposed to a quota window
// that reopens on a clock.
lazy_re!(out_of_credit, r"(?i)\bout of credit|\badd (?:more )?credit|\btop ?up\b");
lazy_re!(insufficient_quota_code, r"(?i)\binsufficient_quota\b");

/// Whether a usage-limit failure is a *billing* cap rather than a window that reopens on a clock:
/// an out-of-credit balance (402), a spent prepaid balance, a credits wall, or a spend ceiling.
/// Topping the account up clears it; waiting never does, so the retry loop must surface the error
/// instead of sleeping the QUOTA_EXHAUSTED heuristic (30 min).
///
/// A DashScope/Bailian per-minute token throttle and a concurrency cap wear quota wording but are
/// transient slots, so both stay out.
pub fn is_billing_cap_usage_limit(status: Option<u32>, message: Option<&str>) -> bool {
    let Some(message) = message else { return false };
    if is_concurrency_cap_exclusion(status, Some(message)) {
        return false;
    }
    if is_dash_scope_token_limit_text(message) {
        return false;
    }
    if status == Some(402) {
        return is_402_billing_cap_body(Some(message));
    }
    out_of_credit().is_match(message)
        || insufficient_balance().is_match(message)
        || credits_exhausted().is_match(message)
        || prepaid_balance_exhausted().is_match(message)
        || spend_limit().is_match(message)
        || anthropic_credits_required().is_match(message)
        || insufficient_quota_code().is_match(message)
}

/// Returns true for failures that should burn one credential and rotate to a sibling account.
/// Decision tree:
///
///  1. Body matches the usage-limit text (Codex `usage_limit_reached`, Anthropic account
///     rate-limit, Google `resource_exhausted`, OpenAI `insufficient_quota`, ...) -> rotate.
///  2. Status is not a usage-limit status (429/402) -> backoff (caller's domain).
///  3. Body is absent or opaque (just the status, empty JSON, HTTP framing only) -> rotate
///     conservatively: the server gave us nothing else to go on.
///  4. Body has content -> defer to [`parse_rate_limit_reason`]. `QUOTA_EXHAUSTED` rotates; for a
///     402 status a `CONCURRENT_LIMIT` body also rotates (the cap is concurrent-worded but the
///     status is an exhausted billing cap). `RATE_LIMIT_EXCEEDED`, `MODEL_CAPACITY_EXHAUSTED`,
///     `SERVER_ERROR`, and `UNKNOWN` stay in the provider's own backoff / failure layer so
///     transient or non-quota responses don't burn sibling credentials.
pub fn is_usage_limit_outcome(status: Option<u32>, message: Option<&str>) -> bool {
    let structured = message.filter(|m| !m.is_empty()).and_then(parse_google_rpc_rate_limit_reason);
    if let Some(reason) = structured {
        return is_quota_exhausted_reason(reason);
    }
    if is_concurrency_cap_exclusion(status, message) {
        return false;
    }
    let msg = message.filter(|m| !m.is_empty());
    if let Some(m) = msg {
        if matches_usage_limit_text(m) {
            return true;
        }
    }
    // A 403 is normally an auth failure, but several providers deliver an account-scoped cap with
    // it (Devin/Codeium Connect `permission_denied`, GitHub Copilot). Devin's end-of-stream Connect
    // trailer carries no HTTP status at all, so accept an undefined status too, but only when the
    // body names a cap that resets, never on a bare 403, which stays an auth failure.
    if status.map_or(true, |s| s == 403) {
        if let Some(m) = msg {
            if is_account_scoped_cap_text(m) {
                return true;
            }
        }
    }
    if status == Some(402) && is_402_billing_cap_body(message) {
        return true;
    }
    if !is_usage_limit_status(status) {
        return false;
    }
    let Some(m) = msg else { return true };
    if is_opaque_status_body(m) {
        return true;
    }
    is_quota_exhausted_reason(parse_rate_limit_reason(m))
}

/// A usage-limit status body is opaque when it carries no signal beyond the status itself: empty,
/// whitespace-only, the status digits with HTTP/JSON framing, or generic punctuation. Anything else
/// (retry hints, capacity wording, error descriptions) is informative enough to defer to the classifier.
pub fn is_opaque_status_body(message: &str) -> bool {
    lazy_re!(status_digits, r"\b(?:429|402)\b");
    lazy_re!(framing_words, r"(?i)\b(?:http|https|status|error|code|response|message)\b");
    lazy_re!(no_body, r"(?i)\(?\bno body\b\)?");
    lazy_re!(informative_word, r"(?i)[a-z\d]{3,}");
    let cleaned = status_digits().replace_all(message, "");
    let cleaned = framing_words().replace_all(&cleaned, "");
    let cleaned = no_body().replace_all(&cleaned, "");
    // A body is informative when the text classifier can act on it. Any Latin word or Simplified
    // Chinese phrasing the classifier recognizes defers to parse_rate_limit_reason; a body that is
    // only status digits / HTTP framing is opaque and rotates conservatively. A Han-only body the
    // classifier cannot interpret must stay opaque so the opaque-429 fallback still rotates.
    !informative_word().is_match(&cleaned)
        && !cn_quota_exhausted().is_match(&cleaned)
        && !cn_transient_cap().is_match(&cleaned)
        && !cn_throttle().is_match(&cleaned)
}

/// Internal text matcher for usage/quota-limit phrasing. `flags` consumes this to populate
/// `Flag::UsageLimit`, and [`is_usage_limit_outcome`] uses it for the account-rotation decision.
pub fn matches_usage_limit_text(error_message: &str) -> bool {
    if let Some(reason) = parse_google_rpc_rate_limit_reason(error_message) {
        return is_quota_exhausted_reason(reason);
    }
    if is_dash_scope_token_limit_text(error_message) {
        return false;
    }
    // Rolling TPM/RPM windows self-heal, so they never rotate a credential. The reason re-check is
    // the precedence guard: an account-scoped cap that merely quotes a TPM number resolves to
    // QUOTA_EXHAUSTED earlier in that ladder and keeps its usage-limit verdict.
    if tpm_rpm_throttle().is_match(error_message)
        && parse_rate_limit_reason(error_message) == RateLimitReason::RateLimitExceeded
    {
        return false;
    }
    usage_limit().is_match(error_message)
        || anthropic_credits_required().is_match(error_message)
        || prepaid_balance_exhausted().is_match(error_message)
        || credits_exhausted().is_match(error_message)
        || (cn_quota_exhausted().is_match(error_message) && !cn_transient_cap().is_match(error_message))
        || spend_limit().is_match(error_message)
        || account_rate_limit().is_match(error_message)
        || matches_subscription_cap_text(error_message)
        || openrouter_daily_free_limit().is_match(error_message)
}

/// Account-scoped cap phrasing delivered on a 403 (or a statusless Connect trailer): "Reached
/// overall message rate limit", "Your limit will reset in ...". Kept separate from
/// [`matches_usage_limit_text`] because the bare wording is ambiguous without the 403 / statusless
/// account context.
pub fn is_account_scoped_cap_text(message: &str) -> bool {
    account_scoped_403().is_match(message)
}

/// A concurrency cap on a non-billing status is shed-and-backoff, not credential-rotatable. A 402
/// remains an account-billing cap.
pub fn is_concurrency_cap_exclusion(status: Option<u32>, message: Option<&str>) -> bool {
    message.map_or(false, |m| parse_rate_limit_reason(m) == RateLimitReason::ConcurrentLimit) && status != Some(402)
}

#[cfg(test)]
mod tests {
    use super::*;
    use RateLimitReason as R;

    #[test]
    fn reasons_follow_the_priority_ladder() {
        assert_eq!(parse_rate_limit_reason("You have exhausted your capacity on this model. Your quota will reset after 2h"), R::QuotaExhausted);
        assert_eq!(parse_rate_limit_reason("Too many concurrent requests"), R::ConcurrentLimit);
        assert_eq!(parse_rate_limit_reason("Service overloaded (529)"), R::ModelCapacityExhausted);
        assert_eq!(parse_rate_limit_reason("rate limit reached, slow down"), R::RateLimitExceeded);
        assert_eq!(parse_rate_limit_reason("monthly spend limit reached"), R::QuotaExhausted);
        assert_eq!(parse_rate_limit_reason("internal server error"), R::ServerError);
        assert_eq!(parse_rate_limit_reason("resource_exhausted"), R::ModelCapacityExhausted);
        assert_eq!(parse_rate_limit_reason("hello"), R::Unknown);
        assert_eq!(parse_rate_limit_reason("使用上限已达到"), R::QuotaExhausted);
        assert_eq!(parse_rate_limit_reason("并发请求数已达上限"), R::Unknown);
    }

    #[test]
    fn dashscope_throttle_is_transient_not_quota() {
        let msg = "429 You exceeded your current quota, please check your plan and billing details. https://help.aliyun.com/error-code#token-limit";
        assert!(is_dash_scope_token_limit_text(msg));
        assert_eq!(parse_rate_limit_reason(msg), R::RateLimitExceeded);
        assert!(!matches_usage_limit_text(msg));
    }

    #[test]
    fn google_rpc_error_info_decides_first() {
        let body = r#"Cloud Code Assist API error (429): {"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"MODEL_CAPACITY_EXHAUSTED"}]}}"#;
        assert_eq!(parse_rate_limit_reason(body), R::ModelCapacityExhausted);
        let quota = r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{"quotaId":"GenerateRequestsPerDayPerProject"}]}]}}"#;
        assert_eq!(parse_rate_limit_reason(quota), R::QuotaExhausted);
        let zero = r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{"quotaValue":"0"}]}]}}"#;
        assert_eq!(parse_rate_limit_reason(zero), R::QuotaExhausted);
    }

    #[test]
    fn opaque_bodies_and_usage_limit_outcome() {
        assert!(is_opaque_status_body("429"));
        assert!(is_opaque_status_body("HTTP 429 (no body)"));
        assert!(!is_opaque_status_body("Too many requests"));
        assert!(is_usage_limit_outcome(Some(429), Some("429")));
        assert!(!is_usage_limit_outcome(Some(429), Some("Too many requests")));
        assert!(is_usage_limit_outcome(Some(429), Some("quota exceeded for this account")));
        assert!(is_usage_limit_outcome(Some(402), None));
        assert!(!is_usage_limit_outcome(Some(500), Some("boom")));
        assert!(is_usage_limit_outcome(Some(403), Some("Reached overall message rate limit")));
        assert!(!is_usage_limit_outcome(Some(403), Some("forbidden")));
    }

    #[test]
    fn concurrency_cap_is_excluded_except_on_402() {
        assert!(is_concurrency_cap_exclusion(Some(429), Some("too many concurrent requests")));
        assert!(!is_concurrency_cap_exclusion(Some(402), Some("too many concurrent requests")));
        assert!(!is_usage_limit_outcome(Some(429), Some("too many concurrent requests")));
    }

    #[test]
    fn billing_caps() {
        assert!(is_billing_cap_usage_limit(Some(402), Some("payment required")));
        assert!(is_billing_cap_usage_limit(Some(429), Some("the organization is out of credit, add more to continue")));
        assert!(!is_billing_cap_usage_limit(Some(429), Some("rate limit reached")));
        assert!(!is_billing_cap_usage_limit(None, None));
    }

    #[test]
    fn backoff_by_reason() {
        assert_eq!(calculate_rate_limit_backoff_ms(R::QuotaExhausted, 0.5), 1_800_000.0);
        assert_eq!(calculate_rate_limit_backoff_ms(R::ConcurrentLimit, 0.5), 5_000.0);
        assert_eq!(calculate_rate_limit_backoff_ms(R::ModelCapacityExhausted, 0.5), 60_000.0);
        assert_eq!(R::ServerError.as_str(), "SERVER_ERROR");
    }
}
