//! Port of `src/error/validation.ts`.

use super::flags::{create, Flag, HasErrorId};

/// Caller-supplied input failed validation before/while building a provider request: bad request
/// body, malformed tool arguments, unsupported content type, a schema that cannot be normalized,
/// an unknown tool, etc.
///
/// This is a programmer/config/contract error, not a transient provider fault: it is never retried.
#[derive(Debug)]
pub struct ValidationError {
    pub message: String,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self { message: message.into(), cause }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}

/// A referenced tool was not found in the active tool set.
#[derive(Debug)]
pub struct ToolNotFoundError {
    pub base: ValidationError,
}

impl ToolNotFoundError {
    pub fn new(tool_name: &str) -> Self {
        Self { base: ValidationError::new(format!("Tool \"{tool_name}\" not found"), None) }
    }
}

impl std::ops::Deref for ToolNotFoundError {
    type Target = ValidationError;
    fn deref(&self) -> &ValidationError {
        &self.base
    }
}
impl std::fmt::Display for ToolNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.base, f)
    }
}
impl std::error::Error for ToolNotFoundError {}

/// Provider/auth configuration was missing or malformed (env var pointing at a missing file,
/// missing projectId, bad bind string, mTLS half-configured, ...).
#[derive(Debug)]
pub struct ConfigurationError {
    pub message: String,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ConfigurationError {
    pub fn new(message: impl Into<String>, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self { message: message.into(), cause }
    }
}

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}

/// A request was abandoned because it exceeded a stream/idle/first-event deadline.
#[derive(Debug)]
pub struct StreamTimeoutError {
    pub message: String,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
    error_id: u32,
}

impl StreamTimeoutError {
    pub fn new(message: Option<&str>, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            message: message.unwrap_or("Request timed out.").to_owned(),
            cause,
            error_id: create(&[Flag::Transient, Flag::Timeout]),
        }
    }
}

impl std::fmt::Display for StreamTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for StreamTimeoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
    }
}
impl HasErrorId for StreamTimeoutError {
    fn error_id(&self) -> Option<u32> {
        Some(self.error_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::is;

    #[test]
    fn messages_and_flags() {
        assert_eq!(ToolNotFoundError::new("grep").to_string(), "Tool \"grep\" not found");
        let t = StreamTimeoutError::new(None, None);
        assert_eq!(t.to_string(), "Request timed out.");
        assert!(is(t.error_id(), Flag::Timeout) && is(t.error_id(), Flag::Transient));
        assert_eq!(ConfigurationError::new("bad", None).to_string(), "bad");
    }
}
