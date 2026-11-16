//! Port of `src/error/retryable.ts`.

use std::sync::OnceLock;

use regex::Regex;

use super::flags::{
    aliyun_mm_gateway_pattern, codex_http_body_read_error_pattern, is_retryable_stream_envelope_error, is_transient_stream_drop_error,
    is_transient_stream_parse_error, is_usage_limit, status, transient_transport_pattern, ErrorLike, ErrorShape,
};
use crate::pi_utils::fetch_retry::{is_retryable_error, is_unexpected_socket_close_message};
use crate::utils::retry_after::{get_headers_from_error, get_should_retry_from_headers};

/// Whether a numeric HTTP status is in the canonical transient/retryable set: 408 (Request
/// Timeout), 429 (Too Many Requests), and any 5xx.
///
/// This is a pure predicate over a status code already in hand, distinct from `classify`, which
/// inspects a whole error (including message text) and may match more. Use this when you only have
/// a `status: number`.
pub fn is_transient_status(status: Option<u32>) -> bool {
    matches!(status, Some(s) if s == 408 || s == 429 || s >= 500)
}

// Provider-stream transient phrasings not covered by the shared transient-transport pattern (TLS
// record corruption, HTTP/2 peer stream errors, upstream code 1302). The shared pattern already
// covers rate-limit / overloaded / 5xx / timeout / first-event wording.
fn provider_transient_extra_pattern() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| Regex::new(r"(?i)bad record mac|stream error.*received from peer|1302").expect("static pattern"))
}

fn is_transient_transport_message(message: &str) -> bool {
    message.contains("tls: bad record mac") || message.contains("type=server_error")
}

/// Whether a provider stream error should be retried against the same credential.
///
/// Account-level usage/quota limits are deliberately treated as **non**-retryable here: they are
/// owned by the credential-rotation layer (auth-gateway / `streamSimple` a/b/c policy), not this
/// seconds-scale provider backoff.
///
/// Every 4xx other than 408/429 is terminal: a request the provider rejected as malformed,
/// unauthorized, or unentitled fails identically on replay.
///
/// A captured `x-should-retry` response header overrides the status and text heuristics below: a
/// server or gateway that already retried and answered `false` must not be replayed by provider
/// retry loops.
pub fn is_provider_retryable_error(error: &dyn ErrorLike) -> bool {
    if error.shape() != ErrorShape::Error {
        return false;
    }
    if is_usage_limit(error, None) {
        return false;
    }
    if let Some(verdict) = get_should_retry_from_headers(get_headers_from_error(error)) {
        return verdict;
    }
    let message = error.message().unwrap_or("");
    if let Some(http_status) = status(error) {
        if (400..500).contains(&http_status) && http_status != 408 && http_status != 429 {
            // Aliyun's multimodal gateway 400s are flaky, not malformed: the same payload succeeds
            // on replay (see the pattern's rationale in flags).
            return aliyun_mm_gateway_pattern().is_match(message);
        }
    }
    let msg = message.to_lowercase();
    if is_unexpected_socket_close_message(&msg)
        || is_transient_transport_message(&msg)
        || transient_transport_pattern().is_match(&msg)
        || codex_http_body_read_error_pattern().is_match(&msg)
        || provider_transient_extra_pattern().is_match(&msg)
        || is_transient_stream_parse_error(error)
        || is_transient_stream_drop_error(error)
        || is_retryable_stream_envelope_error(error)
    {
        return true;
    }
    is_retryable_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classes::{ProviderHttpError, ProviderHttpErrorOptions};
    use crate::error::gateway::GatewayText;
    use reqwest::header::HeaderMap;

    fn http(msg: &str, status: u32) -> ProviderHttpError {
        ProviderHttpError::new(msg, status, ProviderHttpErrorOptions::default())
    }

    #[test]
    fn transient_status_set() {
        assert!(is_transient_status(Some(408)) && is_transient_status(Some(429)) && is_transient_status(Some(503)));
        assert!(!is_transient_status(Some(400)) && !is_transient_status(None));
    }

    #[test]
    fn terminal_4xx_and_usage_limits_are_not_retried() {
        assert!(!is_provider_retryable_error(&http("400 unexpected EOF", 400)));
        assert!(!is_provider_retryable_error(&http("401 unauthorized", 401)));
        assert!(is_provider_retryable_error(&http("408 timeout", 408)));
        assert!(!is_provider_retryable_error(&GatewayText::error("You have hit your ChatGPT usage limit")));
        assert!(is_provider_retryable_error(&http("400 Download multimodal file timed out", 400)));
    }

    #[test]
    fn transient_text_is_retried_and_strings_are_not() {
        assert!(is_provider_retryable_error(&GatewayText::error("upstream connect error or disconnect")));
        assert!(is_provider_retryable_error(&GatewayText::error("stream error received from peer")));
        assert!(!is_provider_retryable_error(&GatewayText::error("something else broke")));
        assert!(!is_provider_retryable_error(&GatewayText::string("socket hang up")));
    }

    #[test]
    fn x_should_retry_overrides_everything() {
        let mut headers = HeaderMap::new();
        headers.insert("x-should-retry", "false".parse().unwrap());
        let e = ProviderHttpError::new("503 overloaded", 503, ProviderHttpErrorOptions { headers: Some(headers), ..Default::default() });
        assert!(!is_provider_retryable_error(&e));
        let mut yes = HeaderMap::new();
        yes.insert("x-should-retry", "true".parse().unwrap());
        let e = ProviderHttpError::new("400 nope", 400, ProviderHttpErrorOptions { headers: Some(yes), ..Default::default() });
        assert!(is_provider_retryable_error(&e));
    }
}
