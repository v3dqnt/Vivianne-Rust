//! Port of `src/error/provider.ts`.

use super::classes::{ProviderHttpError, ProviderHttpErrorOptions};
use super::flags::{create, Flag, HasErrorId};

/// Which part of a provider exchange produced a non-HTTP error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderResponseErrorKind {
    /// Stream closed before a terminal completion/response event.
    IncompleteStream,
    /// Terminal event carried an error / unexpected stop reason.
    Output,
    /// Response body was empty/missing when content was required.
    EmptyBody,
    /// Response completed without actionable output (for example, thoughts only).
    EmptyOutput,
    /// Malformed wire envelope (unexpected message ordering / shape).
    Envelope,
    /// Content was blocked by a provider safety filter.
    ContentBlocked,
    /// Runtime/namespace resolution or other provider-internal failure.
    Runtime,
}

#[derive(Debug, Default)]
pub struct ProviderResponseErrorOptions {
    pub provider: Option<String>,
    pub kind: Option<ProviderResponseErrorKind>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

/// A non-HTTP provider failure: a truncated stream, an error stop reason, an empty body, a
/// malformed envelope, or a runtime fault. For non-2xx HTTP responses use [`ProviderHttpError`]
/// (or a provider subclass) instead.
#[derive(Debug)]
pub struct ProviderResponseError {
    pub message: String,
    pub provider: Option<String>,
    pub kind: ProviderResponseErrorKind,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
    error_id: Option<u32>,
}

impl ProviderResponseError {
    pub fn new(message: impl Into<String>, options: ProviderResponseErrorOptions) -> Self {
        let kind = options.kind.unwrap_or(ProviderResponseErrorKind::Output);
        let error_id = match kind {
            // A safety filter block is terminal and intentionally non-retryable.
            ProviderResponseErrorKind::ContentBlocked => Some(create(&[Flag::ContentBlocked])),
            // A logically empty completed output needs a session-level reminder that asks for the
            // missing final answer. Empty bodies and incomplete streams stay on the generic
            // transient retry/model-fallback path.
            ProviderResponseErrorKind::EmptyOutput => Some(create(&[Flag::Transient, Flag::EmptyResponse])),
            ProviderResponseErrorKind::IncompleteStream | ProviderResponseErrorKind::EmptyBody => {
                Some(create(&[Flag::Transient]))
            }
            _ => None,
        };
        Self { message: message.into(), provider: options.provider, kind, cause: options.cause, error_id }
    }
}

impl ProviderResponseError {
    /// `attach(error, id)`.
    #[must_use]
    pub fn attach(mut self, id: u32) -> Self {
        self.error_id = Some(id);
        self
    }
}

impl std::fmt::Display for ProviderResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProviderResponseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}
impl HasErrorId for ProviderResponseError {
    fn error_id(&self) -> Option<u32> {
        self.error_id
    }
}

macro_rules! provider_http_subclass {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name(pub ProviderHttpError);

        impl $name {
            pub const NAME: &'static str = stringify!($name);
            pub fn new(message: impl Into<String>, status: u32, options: ProviderHttpErrorOptions) -> Self {
                let mut inner = ProviderHttpError::new(message, status, options);
                inner.name = Self::NAME;
                Self(inner)
            }
        }
        impl std::ops::Deref for $name {
            type Target = ProviderHttpError;
            fn deref(&self) -> &ProviderHttpError {
                &self.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl std::error::Error for $name {}
    };
}

provider_http_subclass!(
    /// Non-2xx response from the Devin API.
    DevinApiError
);
provider_http_subclass!(
    /// Non-2xx response from the GitLab Duo direct-access API.
    GitLabDuoApiError
);
provider_http_subclass!(
    /// Non-2xx response from the GitLab Duo Workflow API.
    GitLabDuoWorkflowApiError
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::is;

    #[test]
    fn kinds_pick_their_flags() {
        let mk = |kind| ProviderResponseError::new("m", ProviderResponseErrorOptions { kind: Some(kind), ..Default::default() });
        assert!(is(mk(ProviderResponseErrorKind::ContentBlocked).error_id(), Flag::ContentBlocked));
        let empty = mk(ProviderResponseErrorKind::EmptyOutput);
        assert!(is(empty.error_id(), Flag::Transient) && is(empty.error_id(), Flag::EmptyResponse));
        assert!(is(mk(ProviderResponseErrorKind::IncompleteStream).error_id(), Flag::Transient));
        assert_eq!(mk(ProviderResponseErrorKind::Envelope).error_id(), None);
        assert_eq!(ProviderResponseError::new("x", Default::default()).kind, ProviderResponseErrorKind::Output);
    }

    #[test]
    fn subclass_keeps_status_and_name() {
        let e = DevinApiError::new("nope", 500, Default::default());
        assert_eq!(e.status, 500);
        assert_eq!(e.name, "DevinApiError");
    }
}
