//! Port of `src/error/auth-classify.ts`.

use std::sync::OnceLock;

use regex::Regex;

use super::flags::{
    is_account_policy_error, is_cline_pass_surface_gate_message, is_oauth_expiry, is_usage_limit, ErrorClass, ErrorLike,
    ErrorShape, MessageObject,
};
use super::rate_limit::{is_concurrency_cap_exclusion, is_usage_limit_outcome};
use crate::pi_utils::fetch_retry::extract_http_status_from_error;

/// Whether an OAuth refresh failure is definitive (the credential must be disabled) versus
/// transient. Thin alias over the `Flag::OAuthExpiry` text classifier [`is_oauth_expiry`]; retained
/// as the public `@oh-my-pi/pi-ai` entrypoint name used by the coding agent and auth-broker.
pub fn is_definitive_oauth_failure(error_msg: &str) -> bool {
    is_oauth_expiry(error_msg)
}

fn invalidated_oauth_token_pattern() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| Regex::new(r"(?i)\binvalidated oauth token\b").expect("static pattern"))
}

/// Whether an upstream response explicitly says the supplied OAuth bearer was invalidated.
pub fn is_invalidated_oauth_token_error(error: &dyn ErrorLike) -> bool {
    // `"errorMessage" in error`: the classification message wins when the object carries one.
    if error.error_message().is_some() {
        let text = error.error_classification_message().or(error.error_message());
        if text.map_or(false, |t| invalidated_oauth_token_pattern().is_match(t)) {
            return true;
        }
    }
    let message = match error.shape() {
        ErrorShape::Error | ErrorShape::String => error.message(),
        ErrorShape::Object => None,
    };
    message.map_or(false, |m| invalidated_oauth_token_pattern().is_match(m))
}

/// Whether an upstream failure should retry through the credential resolver: a typed
/// token-refresh request, a hard `401`, a `403` (token valid but access denied: plan, model policy,
/// or org restriction a sibling account may not share), an account-scoped policy denial such as
/// Codex `cyber_policy`, a body-classified usage limit (Codex `usage_limit_reached`, Anthropic
/// account rate-limit, Google `resource_exhausted`, OpenAI `insufficient_quota`, ...), or a bare
/// `429` whose payload did not preserve a richer quota code. Transient 429s (`Too many requests`,
/// per-minute caps) stay in the upstream-backoff lane.
pub fn is_auth_retryable_error(error: &dyn ErrorLike) -> bool {
    if error.class() == ErrorClass::OAuthTokenRefresh {
        return true;
    }
    if is_usage_limit(error, None) {
        return true;
    }
    if is_account_policy_error(error, None) {
        return true;
    }
    if is_invalidated_oauth_token_error(error) {
        return true;
    }
    let mut http_status = extract_http_status_from_error(error);
    let mut message: Option<&str> = match error.shape() {
        ErrorShape::Error | ErrorShape::String => error.message(),
        ErrorShape::Object => None,
    };
    if error.shape() != ErrorShape::String {
        if http_status.is_none() {
            // the `errorStatus` property
            if let Some(s) = error.own_status() {
                http_status = Some(s);
            }
        }
        if message.is_none() {
            message = error.error_message();
        }
    }
    let embedded_status = message
        .filter(|m| !m.is_empty())
        .and_then(|m| extract_http_status_from_error(&MessageObject(m)));
    let status = http_status.or(embedded_status);
    if is_concurrency_cap_exclusion(status, message) {
        return false;
    }
    // A Cline surface-gate 403 is per-model client policy, not a credential problem: sibling keys
    // fail identically, so rotation only burns them.
    if is_cline_pass_surface_gate_message(message) {
        return false;
    }
    if status == Some(401) || status == Some(403) {
        return true;
    }
    is_usage_limit_outcome(status, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::TextError;
    use crate::error::oauth::{OAuthError, OAuthErrorKind, OAuthErrorOptions};

    #[test]
    fn definitive_failure_alias() {
        assert!(is_definitive_oauth_failure("invalid_grant: token revoked"));
        assert!(!is_definitive_oauth_failure("ETIMEDOUT"));
    }

    #[test]
    fn invalidated_token_reads_the_classification_message_first() {
        assert!(is_invalidated_oauth_token_error(&TextError("Invalidated OAuth token")));
        assert!(!is_invalidated_oauth_token_error(&TextError("fine")));
    }

    #[test]
    fn retry_decisions() {
        let refresh = OAuthError::new(
            "x",
            OAuthErrorOptions { kind: Some(OAuthErrorKind::TokenRefresh), ..Default::default() },
        );
        assert!(is_auth_retryable_error(&refresh));
        use crate::error::gateway::GatewayText;
        let err = |m: &str| GatewayText::error(m);
        assert!(is_auth_retryable_error(&err("HTTP 401 unauthorized")));
        assert!(is_auth_retryable_error(&err("HTTP 403 forbidden")));
        assert!(is_auth_retryable_error(&err("HTTP 429")));
        assert!(!is_auth_retryable_error(&err("HTTP 429 Too many requests")));
        assert!(!is_auth_retryable_error(&err("HTTP 403 Only available via Cline product surfaces")));
        assert!(!is_auth_retryable_error(&err("HTTP 500 boom")));
        // A bare `{ message }` object carries no message for this decision, only its status.
        assert!(is_auth_retryable_error(&MessageObject("HTTP 429 Too many requests")));
        assert!(is_auth_retryable_error(&TextError("usage_limit_reached")));
    }
}

