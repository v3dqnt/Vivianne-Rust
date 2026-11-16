//! Port of `src/error/auth.ts`.

use super::flags::{create, Flag, HasErrorId};

/// No API key / credential was available to dispatch a request.
///
/// The default message preserves the historical `"No API key for provider: X"` wording, which
/// [`Flag::AuthFailed`]'s regex (`no api key`) keys off, but the flag is also attached
/// structurally so classification never depends on the exact phrasing.
#[derive(Debug, Clone)]
pub struct MissingApiKeyError {
    pub message: String,
    pub provider: Option<String>,
    error_id: u32,
}

impl MissingApiKeyError {
    pub fn new(provider: Option<&str>, message: Option<&str>) -> Self {
        let message = match (message, provider) {
            (Some(m), _) => m.to_owned(),
            (None, Some(p)) => format!("No API key for provider: {p}"),
            (None, None) => "No API key available".to_owned(),
        };
        Self { message, provider: provider.map(str::to_owned), error_id: create(&[Flag::AuthFailed]) }
    }
}

impl std::fmt::Display for MissingApiKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for MissingApiKeyError {}
impl HasErrorId for MissingApiKeyError {
    fn error_id(&self) -> Option<u32> {
        Some(self.error_id)
    }
}

/// The provider is turned off in its auth rules (`disabled "<reason>"`), so nothing may call it.
#[derive(Debug, Clone)]
pub struct ProviderDisabledError {
    pub base: MissingApiKeyError,
    pub reason: String,
}

impl ProviderDisabledError {
    pub fn new(provider: &str, reason: &str) -> Self {
        let msg = format!("{provider} is turned off: {reason}");
        Self { base: MissingApiKeyError::new(Some(provider), Some(&msg)), reason: reason.to_owned() }
    }
}

impl std::ops::Deref for ProviderDisabledError {
    type Target = MissingApiKeyError;
    fn deref(&self) -> &MissingApiKeyError {
        &self.base
    }
}
impl std::fmt::Display for ProviderDisabledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.base, f)
    }
}
impl std::error::Error for ProviderDisabledError {}
impl HasErrorId for ProviderDisabledError {
    fn error_id(&self) -> Option<u32> {
        self.base.error_id()
    }
}

/// Every usable OAuth credential for a provider failed to refresh with a retryable error
/// (network blip, timeout, 5xx). The stored credentials are intact, so this is classified
/// transient: retrying the request can succeed, unlike [`MissingApiKeyError`]. The refresh failure
/// is kept as `cause`.
#[derive(Debug)]
pub struct OAuthRefreshUnavailableError {
    pub message: String,
    pub provider: String,
    pub cause: Box<dyn std::error::Error + Send + Sync>,
    error_id: u32,
}

impl OAuthRefreshUnavailableError {
    pub fn new(provider: &str, cause: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self {
            message: format!("OAuth token refresh failed for provider {provider}: {cause}"),
            provider: provider.to_owned(),
            cause,
            error_id: create(&[Flag::Transient]),
        }
    }
}

impl std::fmt::Display for OAuthRefreshUnavailableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for OAuthRefreshUnavailableError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
impl HasErrorId for OAuthRefreshUnavailableError {
    fn error_id(&self) -> Option<u32> {
        Some(self.error_id)
    }
}

/// A user-facing login flow required an `onPrompt` callback that was not supplied.
#[derive(Debug, Clone)]
pub struct OnPromptRequiredError {
    pub message: String,
}

impl OnPromptRequiredError {
    pub fn new(provider_label: &str) -> Self {
        Self { message: format!("{provider_label} login requires onPrompt callback") }
    }
}
impl std::fmt::Display for OnPromptRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for OnPromptRequiredError {}

/// An interactive login asked for an API key but the user supplied an empty value.
#[derive(Debug, Clone)]
pub struct ApiKeyRequiredError {
    pub message: String,
}

impl ApiKeyRequiredError {
    pub fn new(message: Option<&str>) -> Self {
        Self { message: message.unwrap_or("API key is required").to_owned() }
    }
}
impl Default for ApiKeyRequiredError {
    fn default() -> Self {
        Self::new(None)
    }
}
impl std::fmt::Display for ApiKeyRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiKeyRequiredError {}

/// A user cancelled an interactive login / device flow. Classified as an abort so it is never
/// surfaced as a retryable transient failure.
#[derive(Debug, Clone)]
pub struct LoginCancelledError {
    pub message: String,
    error_id: u32,
}

impl LoginCancelledError {
    pub fn new(message: Option<&str>) -> Self {
        Self { message: message.unwrap_or("Login cancelled").to_owned(), error_id: create(&[Flag::Abort]) }
    }
}
impl Default for LoginCancelledError {
    fn default() -> Self {
        Self::new(None)
    }
}
impl std::fmt::Display for LoginCancelledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for LoginCancelledError {}
impl HasErrorId for LoginCancelledError {
    fn error_id(&self) -> Option<u32> {
        Some(self.error_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::is;

    #[test]
    fn missing_key_message_and_flag() {
        let e = MissingApiKeyError::new(Some("openai"), None);
        assert_eq!(e.to_string(), "No API key for provider: openai");
        assert!(is(e.error_id(), Flag::AuthFailed));
        assert_eq!(MissingApiKeyError::new(None, None).to_string(), "No API key available");
    }

    #[test]
    fn disabled_provider_keeps_auth_flag() {
        let e = ProviderDisabledError::new("x", "policy");
        assert_eq!(e.to_string(), "x is turned off: policy");
        assert_eq!(e.reason, "policy");
        assert!(is(e.error_id(), Flag::AuthFailed));
    }

    #[test]
    fn refresh_unavailable_is_transient() {
        let e = OAuthRefreshUnavailableError::new("p", "boom".into());
        assert_eq!(e.to_string(), "OAuth token refresh failed for provider p: boom");
        assert!(is(e.error_id(), Flag::Transient));
    }

    #[test]
    fn login_cancel_is_an_abort() {
        assert!(is(LoginCancelledError::default().error_id(), Flag::Abort));
    }
}
