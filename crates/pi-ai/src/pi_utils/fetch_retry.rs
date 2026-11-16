//! Port of the retry-hint half of `@oh-my-pi/pi-utils/fetch-retry` (`packages/utils/src/fetch-retry.ts`).
//!
//! `pi-ai` imports that package for `extractRetryHint` and `isUnexpectedSocketCloseMessage`. Only
//! those are ported here (what `error/` needs). `fetchWithRetry`, `isRetryableError` and the rest
//! of pi-utils come with the modules that use them.

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use reqwest::header::HeaderMap;

use crate::error::flags::{ErrorLike, ErrorShape};

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static pattern"))
}

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            re(&C, $pat)
        }
    };
}

// "reset after 1h2m3s" / "10m15s" / "39s"
lazy_re!(quota_reset, r"(?i)reset after (?:(\d+)h)?(?:(\d+)m)?(\d+(?:\.\d+)?)s");
// "Please retry in 250ms" / "Please retry in 12s"
lazy_re!(please_retry, r"(?i)Please retry in ([0-9.]+)(ms|s)");
// JSON field: "retryDelay": "34.074824224s"
lazy_re!(retry_delay_field, r#"(?i)"retryDelay":\s*"([0-9.]+)(ms|s)""#);
// "try again in 250ms" / "try again in 12s" / "try again in 12sec" / "try again in 5 min" / "~158 min." / "2h" / "90 minutes" / "1 hour"
lazy_re!(
    try_again,
    r"(?i)try again in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)\b"
);
// "Your limit will reset in 13 minutes" / "reset in 13 minutes" / "will reset in 2h". OpenCode Go quota
// errors use "Resets in ..." with day units and compound remainders.
lazy_re!(
    will_reset_in,
    r"(?i)(?:will\s+)?resets?\s+in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)\b"
);
// OpenCode Go compound remainder: "Resets in 2hr 15min".
lazy_re!(
    reset_in_hr_min,
    r"(?i)resets?\s+in\s+~?\s*(\d+(?:\.\d+)?)\s*hr\s*(\d+(?:\.\d+)?)\s*min\b"
);
// "Your limit will reset at 2026-09-01 09:44:51" / "reset at 2026-09-01T09:44:51Z"
lazy_re!(
    will_reset_at,
    r"(?i)(?:will\s+)?reset at\s+([0-9]{4}-[0-9]{2}-[0-9]{2}[ T][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:?[0-9]{2})?)"
);
// Both grammars carry a timezone-naive wall clock. The default reading is UTC; a provider-specific
// offset (Z.AI/Zhipu Beijing time) is applied only through `RetryHintOptions.naive_reset_timezone_offset`,
// never inferred from the language.
lazy_re!(cn_reset_at, r"将在\s*([0-9]{4}-[0-9]{2}-[0-9]{2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2})\s*重置");
// "retry-after-ms=98497000" / "retry-after-ms: 7200000" / "retry-after-ms = 7200000"
lazy_re!(retry_after_ms_body, r"(?i)\bretry-after-ms\s*[:=]\s*([0-9]+)\b");
lazy_re!(offset_suffix, r"(?i)(?:Z|[+-][0-9]{2}:?[0-9]{2})$");
lazy_re!(legacy_retry_after, r"(?i)retry-after\s*[:=]\s*([^\s,;]+)");
lazy_re!(legacy_reset_ms, r"(?i)x-ratelimit-reset-ms\s*[:=]\s*(\d+)");
lazy_re!(legacy_reset, r"(?i)x-ratelimit-reset\s*[:=]\s*(\d+)");

/// Provider-specific interpretation for timezone-naive retry timestamps.
#[derive(Debug, Clone, Default)]
pub struct RetryHintOptions {
    /// UTC offset appended to an absolute reset stamp that omits its timezone.
    pub naive_reset_timezone_offset: Option<String>,
}

fn now_ms() -> f64 {
    Utc::now().timestamp_millis() as f64
}

/// `Date.parse` for the ISO-like stamps this module feeds it, in epoch ms.
fn date_parse_iso(s: &str) -> Option<f64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis() as f64);
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M:%S%z"] {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Some(dt.timestamp_millis() as f64);
        }
    }
    None
}

/// `Date.parse` for an HTTP date (`Retry-After: Wed, 21 Oct 2026 07:28:00 GMT`) or an ISO stamp.
pub(crate) fn date_parse(s: &str) -> Option<f64> {
    if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
        return Some(dt.timestamp_millis() as f64);
    }
    date_parse_iso(s)
}

/// JS `Number(string)`: `None` for NaN.
fn js_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    t.parse::<f64>().ok().filter(|v| !v.is_nan())
}

/// Server-suggested retry delay extraction. Merges the patterns historically used
///
/// Header sources (checked in order):
///  - `retry-after-ms` (milliseconds)
///  - `Retry-After` (numeric seconds, or HTTP date)
///  - `x-ratelimit-reset-ms` (delta ms, or Unix epoch ms/s for large values)
///  - `x-ratelimit-reset` (Unix epoch seconds)
///  - `x-ratelimit-reset-after` (seconds)
///
/// Body patterns:
///  - `Your quota will reset after 18h31m10s` / `10m15s` / `39s`
///  - `Please retry in 250ms` / `Please retry in 12s`
///  - `"retryDelay": "34.074824224s"` (JSON error detail field)
///  - `try again in 250ms` / `try again in 12s` / `try again in 5 min` / `try again in ~158 min`
///  - `retry-after-ms=98497000` / `retry-after-ms: 7200000` / `retry-after-ms = 7200000`
///  - `Your limit will reset at 2026-09-01 09:44:51` / `将在 2026-09-01 09:44:51 重置`
///    (a provider offset makes a naive wall clock authoritative; otherwise it resolves only when
///    no relative signal is present)
///
/// Returns `None` if no signal is found, or `Some(0.0)` when the provider explicitly asks for an
/// immediate retry (`retry-after...=0`, or an absolute reset timestamp that has already elapsed).
pub fn extract_retry_hint(
    headers: Option<&HeaderMap>,
    body: Option<&str>,
    options: Option<&RetryHintOptions>,
) -> Option<f64> {
    if let Some(headers) = headers {
        let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).filter(|v| !v.is_empty());
        if let Some(v) = get("retry-after-ms") {
            if let Some(ms) = js_number(v).filter(|ms| ms.is_finite() && *ms >= 0.0) {
                return Some(ms);
            }
        }
        if let Some(v) = get("retry-after") {
            if let Some(seconds) = js_number(v).filter(|s| s.is_finite()) {
                return Some((seconds * 1000.0).max(0.0));
            }
            if let Some(parsed) = date_parse(v) {
                return Some((parsed - now_ms()).max(0.0));
            }
        }
        if let Some(v) = get("x-ratelimit-reset-ms") {
            if let Some(value) = js_number(v).filter(|x| x.is_finite() && *x > 0.0) {
                // > 1e12 -> epoch ms; > 1e9 -> epoch s; otherwise a delta in ms.
                let target = if value > 1e12 {
                    Some(value)
                } else if value > 1e9 {
                    Some(value * 1000.0)
                } else {
                    None
                };
                match target {
                    None => return Some(value),
                    Some(t) => {
                        let delta = t - now_ms();
                        if delta > 0.0 {
                            return Some(delta);
                        }
                    }
                }
            }
        }
        if let Some(v) = get("x-ratelimit-reset") {
            // parseInt: leading integer digits.
            let digits: String = v.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(reset_seconds) = digits.parse::<f64>() {
                let delta = reset_seconds * 1000.0 - now_ms();
                if delta > 0.0 {
                    return Some(delta);
                }
            }
        }
        if let Some(v) = get("x-ratelimit-reset-after") {
            if let Some(seconds) = js_number(v).filter(|s| s.is_finite() && *s > 0.0) {
                return Some(seconds * 1000.0);
            }
        }
    }

    let body = body.filter(|b| !b.is_empty())?;

    // A body can carry several timing signals at once: the account-reset window plus header
    // timing folded into the message text (already the max across response headers). Honor the
    // longest: retrying before either window clears re-hits a still-blocked credential and burns
    // the retry budget.
    let mut longest_ms: Option<f64> = None;
    // A parsed-but-non-positive signal is a provider "retry now": an explicit `retry-after...=0`
    // or an absolute reset that already elapsed. It must survive as 0 rather than collapse into
    // "no hint found": consumers substitute a heuristic wait when the parse returns None.
    let mut retry_now = false;
    // Timezone-naive `reset at` stamps (no `Z`/offset) are the provider's wall clock in an unknown
    // zone. They resolve AFTER every unambiguous signal below, and only as a fallback.
    let mut longest_naive_ms: Option<f64> = None;

    let mut consider_naive = |ms: Option<f64>, slot: &mut Option<f64>| {
        if let Some(ms) = ms {
            if ms > 0.0 && slot.map_or(true, |cur| ms > cur) {
                *slot = Some(ms);
            }
        }
    };
    fn consider(ms: Option<f64>, longest: &mut Option<f64>) {
        if let Some(ms) = ms {
            if ms > 0.0 && longest.map_or(true, |cur| ms > cur) {
                *longest = Some(ms);
            }
        }
    }

    if let Some(c) = quota_reset().captures(body) {
        let hours = c.get(1).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
        let minutes = c.get(2).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
        if let Ok(seconds) = c[3].parse::<f64>() {
            let total = ((hours * 60.0 + minutes) * 60.0 + seconds) * 1000.0;
            consider(if total > 0.0 { Some(total) } else { None }, &mut longest_ms);
        }
    }
    for pattern in [will_reset_at(), cn_reset_at()] {
        let Some(c) = pattern.captures(body) else { continue };
        let Some(m) = c.get(1) else { continue };
        // Offset-bearing stamps are unambiguous and compete by longest-wins. A configured provider
        // offset makes an otherwise naive wall clock unambiguous too. Without one, preserve the
        // relative-signal-first fallback and interpret the wall clock as UTC only when it stands alone.
        let normalized = m.as_str().replacen(' ', "T", 1);
        let has_offset = offset_suffix().is_match(&normalized);
        let configured = options.and_then(|o| o.naive_reset_timezone_offset.as_deref());
        let candidate = if has_offset {
            normalized.clone()
        } else {
            format!("{normalized}{}", configured.unwrap_or("Z"))
        };
        if let Some(parsed) = date_parse_iso(&candidate) {
            let delta = parsed - now_ms();
            if parsed > now_ms() {
                if has_offset || configured.is_some() {
                    consider(Some(delta), &mut longest_ms);
                } else {
                    consider_naive(Some(delta), &mut longest_naive_ms);
                }
            }
        }
    }
    // OpenCode Go compound remainder ("Resets in 2hr 15min"): the generic pattern only captures
    // the leading "2hr", so add the trailing minutes.
    if let Some(c) = reset_in_hr_min().captures(body) {
        if let (Ok(hours), Ok(minutes)) = (c[1].parse::<f64>(), c[2].parse::<f64>()) {
            if hours >= 0.0 && minutes > 0.0 {
                consider(Some(hours * 60.0 * 60_000.0 + minutes * 60_000.0), &mut longest_ms);
            }
        }
    }
    if let Some(c) = will_reset_in().captures(body) {
        if let Ok(value) = c[1].parse::<f64>() {
            if value.is_finite() && value > 0.0 {
                if let Some(unit) = unit_to_ms(&c[2]) {
                    consider(Some(value * unit), &mut longest_ms);
                }
            }
        }
    }

    let mut consider_clamped = |ms: Option<f64>, longest: &mut Option<f64>, retry_now: &mut bool| {
        if let Some(ms) = ms {
            if ms > 0.0 {
                consider(Some(ms), longest);
            } else {
                *retry_now = true;
            }
        }
    };

    if let Some(c) = retry_after_ms_body().captures(body) {
        if let Ok(ms) = c[1].parse::<f64>() {
            consider_clamped(Some(ms), &mut longest_ms, &mut retry_now);
        }
    }

    for pattern in [please_retry(), retry_delay_field(), try_again()] {
        if let Some(c) = pattern.captures(body) {
            if let Ok(value) = c[1].parse::<f64>() {
                if value.is_finite() && value > 0.0 {
                    if let Some(unit) = unit_to_ms(&c[2]) {
                        consider(Some(value * unit), &mut longest_ms);
                    }
                }
            }
        }
    }

    // Legacy text forms (also honored by older local parsers): a plain `retry-after` delta in
    // seconds or as an HTTP date, and `x-ratelimit-reset[-ms]` counters. They compete in the same
    // maximum: a longer legacy hint must not lose to a shorter reset phrase. Their non-positive
    // readings (explicit zero, elapsed epoch/date) are the clamped retry-now signals above.
    if let Some(c) = legacy_retry_after().captures(body) {
        let value = &c[1];
        match js_number(value).filter(|s| s.is_finite()) {
            Some(seconds) => consider_clamped(Some(seconds * 1000.0), &mut longest_ms, &mut retry_now),
            None => {
                if let Some(date_ms) = date_parse(value) {
                    consider_clamped(Some(date_ms - now_ms()), &mut longest_ms, &mut retry_now);
                }
            }
        }
    }
    if let Some(c) = legacy_reset_ms().captures(body) {
        if let Ok(reset_ms) = c[1].parse::<f64>() {
            let ms = if reset_ms > 1_000_000_000_000.0 { reset_ms - now_ms() } else { reset_ms };
            consider_clamped(Some(ms), &mut longest_ms, &mut retry_now);
        }
    }
    if let Some(c) = legacy_reset().captures(body) {
        if let Ok(reset_seconds) = c[1].parse::<f64>() {
            let ms = if reset_seconds > 1_000_000_000.0 {
                reset_seconds * 1000.0 - now_ms()
            } else {
                reset_seconds * 1000.0
            };
            consider_clamped(Some(ms), &mut longest_ms, &mut retry_now);
        }
    }
    // Elapsed naive stamps were ignored before this change and stay ignored: only an explicit
    // zero/expired relative signal is authoritative retry-now.
    longest_ms.or(longest_naive_ms).or(if retry_now { Some(0.0) } else { None })
}


// `extractStatusFromMessage` patterns (distinct from the pi-ai `status()` set).
lazy_re!(status_msg_error_colon, r"(?i)\berror\s*[:=]\s*(\d{3})\b");
lazy_re!(status_msg_error_paren, r"(?i)error\s*\((\d{3})\)");
lazy_re!(status_msg_status, r"(?i)status\s*[:=]?\s*(\d{3})");
lazy_re!(status_msg_http, r"(?i)\bhttp\s*(\d{3})\b");
lazy_re!(status_msg_trailing, r"(?i)\b(\d{3})\s*(?:status|error)\b");

fn extract_status_from_message(message: &str) -> Option<u32> {
    for pattern in [
        status_msg_error_colon(),
        status_msg_error_paren(),
        status_msg_status(),
        status_msg_http(),
        status_msg_trailing(),
    ] {
        if let Some(c) = pattern.captures(message) {
            if let Ok(value) = c[1].parse::<u32>() {
                if (100..=599).contains(&value) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// Inspect an arbitrary error value (or its `cause` chain, up to depth 2) for an HTTP status code.
/// Reads `status`, `statusCode`, and `response.status` fields, coerces string values, and falls
/// back to scanning the error message for common patterns like `Error: 401`, `error (429)`, or
/// `HTTP 503`. A bare string is not an object, so it yields `None`.
pub fn extract_http_status_from_error(error: &dyn ErrorLike) -> Option<u32> {
    extract_http_status_internal(error, 0)
}

fn extract_http_status_internal(error: &dyn ErrorLike, depth: u32) -> Option<u32> {
    if error.shape() == ErrorShape::String || depth > 2 {
        return None;
    }
    if let Some(s) = error.own_status().filter(|s| (100..=599).contains(s)) {
        return Some(s);
    }
    if let Some(m) = error.message().filter(|m| !m.is_empty()) {
        if let Some(extracted) = extract_status_from_message(m) {
            return Some(extracted);
        }
    }
    error.cause().and_then(|c| extract_http_status_internal(c, depth + 1))
}

/// `true` if the given HTTP status code is one we treat as transient: 408 (Request Timeout), 429
/// (Too Many Requests), or any 5xx (server error).
pub fn is_retryable_status(status: u32) -> bool {
    status >= 500 || status == 408 || status == 429
}


lazy_re!(
    transient_message,
    r"(?i)overloaded|rate.?limit|too many requests|service.?unavailable|server error|internal error|connection.?error|unable to connect|fetch failed|network error|stream stall|other side closed|HTTP2(?:StreamReset|RefusedStream|EnhanceYourCalm)"
);
lazy_re!(
    validation_message,
    r"(?i)invalid|validation|bad request|unsupported|schema|missing required|not found|unauthorized|forbidden"
);
lazy_re!(abort_or_timeout_message, r"(?i)timeout|timed out|aborted");

/// Identify errors that should be retried: aborts/timeouts in the error name or message, retryable
/// HTTP statuses (see [`is_retryable_status`]), unexpected socket closes, and the standard
/// transient phrases. 4xx statuses other than 408/429 and validation-shaped messages
/// short-circuit to `false`.
pub fn is_retryable_error(error: &dyn ErrorLike) -> bool {
    let message = error.message().unwrap_or("");
    let name = error.name().unwrap_or("");
    if name == "AbortError" || abort_or_timeout_message().is_match(message) {
        return true;
    }
    if let Some(status) = extract_http_status_from_error(error) {
        if is_retryable_status(status) {
            return true;
        }
        if (400..500).contains(&status) {
            return false;
        }
    }
    if validation_message().is_match(message) {
        return false;
    }
    is_unexpected_socket_close_message(message) || transient_message().is_match(message)
}

pub fn unit_to_ms(unit: &str) -> Option<f64> {
    match unit.to_lowercase().as_str() {
        "ms" => Some(1.0),
        "s" | "sec" => Some(1000.0),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(60_000.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(60.0 * 60_000.0),
        "d" | "day" | "days" => Some(24.0 * 60.0 * 60_000.0),
        _ => None,
    }
}

pub fn is_unexpected_socket_close_message(message: &str) -> bool {
    static A: OnceLock<Regex> = OnceLock::new();
    static B: OnceLock<Regex> = OnceLock::new();
    re(&A, r"(?i)\b(?:the\s+)?socket connection (?:was )?closed unexpectedly\b").is_match(message)
        || re(&B, r"(?i)^(?:error:\s*)?socket is closed\.?$").is_match(message.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::{MessageObject, TextError};

    #[test]
    fn http_status_from_object_message_and_not_strings() {
        assert_eq!(extract_http_status_from_error(&MessageObject("HTTP 503 upstream")), Some(503));
        assert_eq!(extract_http_status_from_error(&MessageObject("error (429)")), Some(429));
        assert_eq!(extract_http_status_from_error(&TextError("HTTP 503")), None);
        assert!(is_retryable_status(502) && is_retryable_status(408) && !is_retryable_status(404));
    }


    fn hint(body: &str) -> Option<f64> {
        extract_retry_hint(None, Some(body), None)
    }

    #[test]
    fn parses_relative_phrases() {
        assert_eq!(hint("Please retry in 250ms"), Some(250.0));
        assert_eq!(hint("try again in 5 min"), Some(300_000.0));
        assert_eq!(hint("quota will reset after 1h2m3s"), Some(3_723_000.0));
        assert_eq!(hint(r#""retryDelay": "34s""#), Some(34_000.0));
        assert_eq!(hint("Resets in 2hr 15min"), Some(8_100_000.0));
    }

    #[test]
    fn longest_signal_wins_and_zero_means_retry_now() {
        assert_eq!(hint("Please retry in 2s. try again in 10s"), Some(10_000.0));
        assert_eq!(hint("retry-after-ms=0"), Some(0.0));
        assert_eq!(hint("nothing useful"), None);
    }

    #[test]
    fn headers_take_precedence() {
        let mut h = HeaderMap::new();
        h.insert("retry-after-ms", "1500".parse().unwrap());
        assert_eq!(extract_retry_hint(Some(&h), Some("try again in 1h"), None), Some(1500.0));
        let mut h2 = HeaderMap::new();
        h2.insert("retry-after", "3".parse().unwrap());
        assert_eq!(extract_retry_hint(Some(&h2), None, None), Some(3000.0));
    }

    #[test]
    fn socket_close_detector() {
        assert!(is_unexpected_socket_close_message("The socket connection was closed unexpectedly"));
        assert!(is_unexpected_socket_close_message("Error: socket is closed."));
        assert!(!is_unexpected_socket_close_message("socket is closed by peer"));
    }
}
