//! Port of `src/error/abort.ts`.

use tokio_util::sync::CancellationToken;

use super::flags::{create, Flag, HasErrorId};

/// A request was cancelled: by the caller's abort signal or a provider-local watchdog. Carries
/// the [`Flag::Abort`] classification structurally so retry logic does not have to regex the
/// message text.
///
/// The default message is kept byte-identical to the historical `"Request was aborted"` string so
/// any remaining text-based matchers keep working through the migration.
#[derive(Debug, Clone)]
pub struct AbortError {
    pub message: String,
    pub cause: Option<String>,
    error_id: u32,
}

impl AbortError {
    pub const DEFAULT_MESSAGE: &'static str = "Request was aborted";

    pub fn new(message: Option<&str>, cause: Option<String>) -> Self {
        Self {
            message: message.unwrap_or(Self::DEFAULT_MESSAGE).to_owned(),
            cause,
            error_id: create(&[Flag::Abort]),
        }
    }
}

impl Default for AbortError {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl std::fmt::Display for AbortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AbortError {}

impl HasErrorId for AbortError {
    fn error_id(&self) -> Option<u32> {
        Some(self.error_id)
    }
}

/// The text a caller gave `AbortController.abort(reason)`: a string or an `Error` other than an
/// abort, such as the coding agent's user-interrupt label; `None` for a bare `abort()`, whose
/// reason is the default `AbortError`.
///
/// A `CancellationToken` carries no reason, so callers hand the reason they recorded beside it.
pub fn caller_abort_reason(signal: Option<&CancellationToken>, reason: Option<&str>) -> Option<String> {
    signal?;
    let reason = reason?;
    if reason.trim().is_empty() {
        return None;
    }
    Some(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_message_and_flag() {
        let e = AbortError::default();
        assert_eq!(e.to_string(), "Request was aborted");
        assert!(super::super::flags::is(e.error_id(), Flag::Abort));
    }

    #[test]
    fn blank_reason_is_none() {
        let t = CancellationToken::new();
        assert_eq!(caller_abort_reason(Some(&t), Some("  ")), None);
        assert_eq!(caller_abort_reason(Some(&t), Some("user stop")), Some("user stop".into()));
        assert_eq!(caller_abort_reason(None, Some("x")), None);
    }
}
