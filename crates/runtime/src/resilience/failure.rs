//! What a failed model call means for the loop: try again, try something else, or give up.

use vivianne_ai::ai::utils::{CredentialErrorClass, classify_provider_error, truncate_error_text};
use vivianne_providers::ProviderError;

/// Longest error text shown to the user or put in an event.
const REASON_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// A rate limit, a 5xx, a timeout or a dropped connection: the same call may work in a moment.
    Transient,
    /// This key or plan is out of quota or credit: waiting won't help, another key or model might.
    OutOfQuota,
    /// Anything else (bad key, bad request, unknown model): retrying the same call is pointless.
    Permanent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl Failure {
    #[must_use]
    pub fn from_message(message: &str) -> Self {
        let kind = match classify_provider_error(message) {
            CredentialErrorClass::Retryable => FailureKind::Transient,
            CredentialErrorClass::QuotaOrBilling => FailureKind::OutOfQuota,
            CredentialErrorClass::Other => FailureKind::Permanent,
        };
        Self {
            kind,
            message: message.to_owned(),
        }
    }

    /// Whether the same call is worth repeating.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        self.kind == FailureKind::Transient
    }

    /// Whether a different key or model is worth trying (everything but a permanent, call-shaped
    /// failure). A bad request fails the same way on every model, so it stops the chain.
    #[must_use]
    pub fn is_worth_another_target(&self) -> bool {
        self.kind != FailureKind::Permanent || mentions_credentials(&self.message)
    }

    /// Whether the request did not fit the model's context window: a different model of the same size fails the
    /// same way, but a shorter conversation works.
    #[must_use]
    pub fn is_context_overflow(&self) -> bool {
        let lower = self.message.to_lowercase();
        [
            "context length",
            "context_length",
            "context window",
            "maximum context",
            "prompt is too long",
            "too many tokens",
            "request too large",
            "exceeds the limit",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    /// The message cut short enough to show.
    #[must_use]
    pub fn short(&self) -> String {
        truncate_error_text(&self.message, REASON_CHARS)
    }
}

impl From<&ProviderError> for Failure {
    fn from(error: &ProviderError) -> Self {
        match error {
            // The provider could not even be set up: another target may still work.
            ProviderError::Unavailable(message) => Self {
                kind: FailureKind::OutOfQuota,
                message: message.clone(),
            },
            ProviderError::Failed(message) => Self::from_message(message),
        }
    }
}

/// A rejected or missing key is specific to that key, so another target may well work.
fn mentions_credentials(message: &str) -> bool {
    let lower = message.to_lowercase();
    [
        "api key",
        "api_key",
        "unauthorized",
        "401",
        "403",
        "forbidden",
        "authentication",
        "invalid x-api-key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_limit_is_transient() {
        assert!(Failure::from_message("429 Too Many Requests: rate limit exceeded").is_transient());
    }

    #[test]
    fn a_bad_request_is_permanent_and_stops_the_chain() {
        let failure = Failure::from_message("400 invalid request: messages must not be empty");
        assert_eq!(failure.kind, FailureKind::Permanent);
        assert!(!failure.is_worth_another_target());
    }

    #[test]
    fn a_request_that_does_not_fit_the_window_is_recognised() {
        assert!(
            Failure::from_message("400: This model's maximum context length is 8192 tokens")
                .is_context_overflow()
        );
        assert!(
            Failure::from_message("prompt is too long: 250000 tokens > 200000 maximum")
                .is_context_overflow()
        );
        assert!(!Failure::from_message("429 rate limit exceeded").is_context_overflow());
    }

    #[test]
    fn a_rejected_key_is_permanent_for_that_key_but_worth_another_target() {
        let failure = Failure::from_message("401 Unauthorized: invalid API key");
        assert!(!failure.is_transient());
        assert!(failure.is_worth_another_target());
    }

    #[test]
    fn a_provider_that_cannot_be_set_up_is_worth_another_target() {
        let failure = Failure::from(&ProviderError::Unavailable("no base URL".into()));
        assert!(failure.is_worth_another_target());
    }

    #[test]
    fn the_short_form_never_splits_a_character() {
        let failure = Failure::from_message(&"é".repeat(500));
        assert!(failure.short().starts_with("éé"));
    }
}
