//! Port of `src/error/aws.ts`.

/// Which AWS credential-resolution path failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsCredentialsErrorKind {
    /// No usable credential source resolved (chain exhausted).
    Resolution,
    /// SSO cache token missing (`aws sso login` not run).
    SsoTokenMissing,
    /// SSO cache token present but expired.
    SsoTokenExpired,
    /// SSO `GetRoleCredentials` call failed or returned no role.
    SsoRole,
    /// External `credential_process` failed, timed out, or emitted bad output.
    CredentialProcess,
    /// STS web-identity exchange failed or returned malformed credentials.
    WebIdentity,
    /// ECS/container credential endpoint failed or returned malformed credentials.
    Container,
    /// Shared-config role chain is misconfigured (cycle, missing source_profile, unsupported credential_source).
    Profile,
    /// STS `AssumeRole` call failed or returned malformed credentials.
    AssumeRole,
}

impl AwsCredentialsErrorKind {
    /// The TS string literal.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolution => "resolution",
            Self::SsoTokenMissing => "sso-token-missing",
            Self::SsoTokenExpired => "sso-token-expired",
            Self::SsoRole => "sso-role",
            Self::CredentialProcess => "credential-process",
            Self::WebIdentity => "web-identity",
            Self::Container => "container",
            Self::Profile => "profile",
            Self::AssumeRole => "assume-role",
        }
    }
}

/// A failure resolving AWS credentials for the Bedrock provider.
#[derive(Debug, Clone)]
pub struct AwsCredentialsError {
    pub message: String,
    pub kind: AwsCredentialsErrorKind,
    pub cause: Option<String>,
}

impl AwsCredentialsError {
    pub fn new(message: impl Into<String>, kind: AwsCredentialsErrorKind, cause: Option<String>) -> Self {
        Self { message: message.into(), kind, cause }
    }
}

impl std::fmt::Display for AwsCredentialsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AwsCredentialsError {}

/// A malformed AWS event-stream frame (bad length, CRC mismatch, unknown header type).
#[derive(Debug, Clone)]
pub struct EventStreamFrameError {
    pub message: String,
}

impl EventStreamFrameError {
    pub fn new(detail: &str) -> Self {
        Self { message: format!("eventstream: {detail}") }
    }
}

impl std::fmt::Display for EventStreamFrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EventStreamFrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_error_prefixes_detail() {
        assert_eq!(EventStreamFrameError::new("bad crc").to_string(), "eventstream: bad crc");
        assert_eq!(AwsCredentialsErrorKind::SsoTokenExpired.as_str(), "sso-token-expired");
    }
}
