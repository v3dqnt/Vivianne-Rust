//! Port of `src/error/oauth.ts`.

use super::flags::{create, Flag, HasErrorId};

/// What stage of an OAuth / device-code login flow failed. Discriminates the single
/// [`OAuthError`] class so login flows don't each mint a bespoke error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthErrorKind {
    /// Token-exchange / refresh / discovery HTTP response was non-2xx or unparseable.
    Http,
    /// Response body was missing required fields (token, account id, endpoints, ...).
    Validation,
    /// Authorization-code to token exchange failed.
    TokenExchange,
    /// Refresh-token grant failed.
    TokenRefresh,
    /// Device-code / authorization polling failed (server error, too many retries).
    Polling,
    /// The flow exceeded its deadline (device-code expiry, polling timeout).
    Timeout,
    /// Device authorization was denied or cancelled by the user/provider.
    DeviceAuth,
    /// Misconfiguration (bad redirect URI, missing projectId, callback bind, ...).
    Configuration,
    /// Cloud project provisioning / onboarding (loadCodeAssist, onboardUser).
    Provisioning,
    /// Subscription/payment gate: plan required or purchase action needed.
    Entitlement,
    /// OIDC / endpoint discovery failed.
    Discovery,
}

#[derive(Debug, Default)]
pub struct OAuthErrorOptions {
    pub kind: Option<OAuthErrorKind>,
    pub provider: Option<String>,
    pub status: Option<u32>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

/// A failure inside an interactive OAuth / device-code login flow. The `kind` pinpoints the
/// stage. Timeout/polling are classified transient; everything else is a hard auth failure. The
/// auth-retry classifier separately treats `token-refresh` as an explicit request to refresh and
/// replay once.
#[derive(Debug)]
pub struct OAuthError {
    pub message: String,
    pub kind: OAuthErrorKind,
    pub provider: Option<String>,
    pub status: Option<u32>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
    error_id: u32,
}

impl OAuthError {
    pub fn new(message: impl Into<String>, options: OAuthErrorOptions) -> Self {
        let kind = options.kind.unwrap_or(OAuthErrorKind::Http);
        let flag = if matches!(kind, OAuthErrorKind::Timeout | OAuthErrorKind::Polling) {
            Flag::Transient
        } else {
            Flag::AuthFailed
        };
        Self {
            message: message.into(),
            kind,
            provider: options.provider,
            status: options.status,
            cause: options.cause,
            error_id: create(&[flag]),
        }
    }
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for OAuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}
impl HasErrorId for OAuthError {
    fn error_id(&self) -> Option<u32> {
        Some(self.error_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::is;

    #[test]
    fn timeout_and_polling_are_transient_the_rest_are_auth_failures() {
        let t = OAuthError::new("t", OAuthErrorOptions { kind: Some(OAuthErrorKind::Timeout), ..Default::default() });
        assert!(is(t.error_id(), Flag::Transient));
        let h = OAuthError::new("h", OAuthErrorOptions::default());
        assert_eq!(h.kind, OAuthErrorKind::Http);
        assert!(is(h.error_id(), Flag::AuthFailed));
    }
}
