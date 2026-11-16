//! Port of `src/utils/retry-after.ts`.

use reqwest::header::HeaderMap;

use crate::error::flags::{ErrorLike, ErrorShape};
use crate::pi_utils::fetch_retry::{date_parse, extract_retry_hint, RetryHintOptions};

/// `Headers | Record<string, string | undefined> | undefined | null`: the header record form is
/// covered by building a [`HeaderMap`].
pub type HeadersLike<'a> = Option<&'a HeaderMap>;

/// Extracts retry timing using the provider's catalog-declared timestamp timezone.
///
/// `retry_reset_timezone_offset_for` is the catalog's `retryResetTimezoneOffsetFor(provider)`
/// lookup (`compat/behavior`), supplied by the caller until the catalog is ported.
pub fn extract_provider_retry_hint(
    provider: Option<&str>,
    message: Option<&str>,
    retry_reset_timezone_offset_for: &dyn Fn(&str) -> Option<String>,
) -> Option<f64> {
    let naive_reset_timezone_offset = provider.and_then(retry_reset_timezone_offset_for);
    extract_retry_hint(None, message, Some(&RetryHintOptions { naive_reset_timezone_offset }))
}

const RETRY_AFTER_HINT: &str = "retry-after-ms=";

/// `error instanceof Error ? error.message : JSON.stringify(error)`. A string error serializes as
/// a quoted JSON string; a bare object carries only its message here.
pub fn error_message_text(error: &dyn ErrorLike) -> String {
    match error.shape() {
        ErrorShape::Error => error.message().unwrap_or("").to_owned(),
        ErrorShape::String => serde_json::to_string(error.message().unwrap_or("")).unwrap_or_default(),
        ErrorShape::Object => serde_json::json!({ "message": error.message() }).to_string(),
    }
}

pub fn append_retry_after_hint(message: &str, headers: HeadersLike<'_>) -> String {
    if message.contains(RETRY_AFTER_HINT) {
        return message.to_owned();
    }
    match get_retry_after_ms_from_headers(headers) {
        None => message.to_owned(),
        Some(ms) => format!("{message} {RETRY_AFTER_HINT}{}", js_number_to_string(ms)),
    }
}

/// JS `String(number)` for the whole-millisecond values this module produces.
fn js_number_to_string(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e21 {
        format!("{}", v as i64)
    } else {
        v.to_string()
    }
}

pub fn format_error_message_with_retry_after(error: &dyn ErrorLike) -> String {
    append_retry_after_hint(&error_message_text(error), get_headers_from_error(error))
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

pub fn get_retry_after_ms_from_headers(headers: HeadersLike<'_>) -> Option<f64> {
    let headers = headers?;
    let retry_after_ms = parse_retry_after_ms_header(header_value(headers, "retry-after-ms"));
    let retry_after = parse_retry_after_header(header_value(headers, "retry-after"));
    let reset_ms = parse_reset_header(header_value(headers, "x-ratelimit-reset-ms"), ResetUnit::Ms);
    let reset_seconds = parse_reset_header(header_value(headers, "x-ratelimit-reset"), ResetUnit::S);

    [retry_after_ms, retry_after, reset_ms, reset_seconds]
        .into_iter()
        .flatten()
        .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))))
}

/// The server's explicit `x-should-retry` verdict: `Some(true)`/`Some(false)` when the header says
/// so, `None` when absent or unrecognised. Non-standard (Stainless-SDK convention) but emitted by
/// OpenAI, Anthropic, and retrying gateways; when present it overrides status/text retry heuristics.
pub fn get_should_retry_from_headers(headers: HeadersLike<'_>) -> Option<bool> {
    match header_value(headers?, "x-should-retry") {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

pub fn get_headers_from_error(error: &dyn ErrorLike) -> HeadersLike<'_> {
    if error.shape() == ErrorShape::String {
        return None;
    }
    if let Some(h) = error.headers() {
        return Some(h);
    }
    error.cause().and_then(get_headers_from_error)
}

fn js_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    t.parse::<f64>().ok().filter(|v| !v.is_nan())
}

fn now_ms() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

/// `retry-after-ms` (Anthropic-style): a plain millisecond delta.
fn parse_retry_after_ms_header(value: Option<&str>) -> Option<f64> {
    let value = value.filter(|v| !v.is_empty())?;
    let ms = js_number(value.trim()).filter(|ms| ms.is_finite())?;
    if ms <= 0.0 {
        return None;
    }
    Some(ms.ceil())
}

fn parse_retry_after_header(value: Option<&str>) -> Option<f64> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(numeric) = js_number(trimmed).filter(|n| n.is_finite()) {
        if numeric <= 0.0 {
            return None;
        }
        return Some((numeric * 1000.0).ceil());
    }
    let date_ms = date_parse(trimmed)?;
    let delay = date_ms - now_ms();
    if delay > 0.0 { Some(delay.ceil()) } else { None }
}

#[derive(Clone, Copy)]
enum ResetUnit {
    Ms,
    S,
}

fn parse_reset_header(value: Option<&str>, unit: ResetUnit) -> Option<f64> {
    let value = value.filter(|v| !v.is_empty())?;
    let numeric = js_number(value).filter(|n| n.is_finite() && *n > 0.0)?;
    let now = now_ms();
    let target_ms = if numeric > 1e12 {
        numeric
    } else if numeric > 1e9 {
        numeric * 1000.0
    } else {
        return Some(match unit {
            ResetUnit::Ms => numeric.ceil(),
            ResetUnit::S => (numeric * 1000.0).ceil(),
        });
    };
    if target_ms <= now {
        return None;
    }
    Some((target_ms - now).ceil())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::TextError;
    use crate::error::classes::{ProviderHttpError, ProviderHttpErrorOptions};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn takes_the_longest_header_signal() {
        let h = headers(&[("retry-after-ms", "1500.2"), ("retry-after", "3"), ("x-ratelimit-reset", "7")]);
        assert_eq!(get_retry_after_ms_from_headers(Some(&h)), Some(7000.0));
        assert_eq!(get_retry_after_ms_from_headers(Some(&headers(&[("retry-after", "0")]))), None);
        assert_eq!(get_retry_after_ms_from_headers(None), None);
    }

    #[test]
    fn hint_is_appended_once() {
        let h = headers(&[("retry-after", "2")]);
        assert_eq!(append_retry_after_hint("429 slow", Some(&h)), "429 slow retry-after-ms=2000");
        assert_eq!(append_retry_after_hint("x retry-after-ms=5", Some(&h)), "x retry-after-ms=5");
        assert_eq!(append_retry_after_hint("429 slow", None), "429 slow");
    }

    #[test]
    fn should_retry_verdict() {
        assert_eq!(get_should_retry_from_headers(Some(&headers(&[("x-should-retry", "false")]))), Some(false));
        assert_eq!(get_should_retry_from_headers(Some(&headers(&[("x-should-retry", "maybe")]))), None);
    }

    #[test]
    fn error_headers_walk_the_cause_and_message_text() {
        let e = ProviderHttpError::new(
            "429 busy",
            429,
            ProviderHttpErrorOptions { headers: Some(headers(&[("retry-after-ms", "900")])), ..Default::default() },
        );
        assert_eq!(format_error_message_with_retry_after(&e), "429 busy retry-after-ms=900");
        assert_eq!(error_message_text(&TextError("boom")), "\"boom\"");
    }

    #[test]
    fn provider_hint_uses_the_supplied_offset_lookup() {
        let lookup = |p: &str| if p == "zai" { Some("+08:00".to_owned()) } else { None };
        assert_eq!(extract_provider_retry_hint(Some("x"), Some("try again in 5s"), &lookup), Some(5000.0));
        assert_eq!(extract_provider_retry_hint(None, None, &lookup), None);
    }
}
