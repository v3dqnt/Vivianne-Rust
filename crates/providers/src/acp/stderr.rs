//! What an agent wrote on stderr, cleaned for display: a failure can say what the agent said without leaking
//! the home directory, a sign-in link or a token.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use super::conn::Conn;

/// How long a failure waits for the agent's last stderr lines, which arrive just after the exit that caused it.
const DRAIN_WAIT: Duration = Duration::from_millis(250);

/// The agent's stderr with anything private taken out of it.
#[must_use]
pub(super) fn sanitize(text: &str) -> String {
    static PAIRING: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"https?://[^\s]*/pair#[^\s]*").ok());
    static SIGN_IN: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"https?://accounts\.google\.com[^\s]*").ok());
    static BEARER: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"\bBearer\s+[A-Za-z0-9._\-+=/]+").ok());
    static BASIC: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"\bAuthorization:\s*Basic\s+\S+").ok());
    static API_KEY: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"\bx-api-key:\s*\S+").ok());
    static SECRET: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r"\b(?:sk-[A-Za-z0-9][A-Za-z0-9_-]{7,}|ghp_[A-Za-z0-9]+|xox[a-zA-Z]-[A-Za-z0-9-]+)\b",
        )
        .ok()
    });

    let mut out = text.replace('\0', "");
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(key)
            && home.chars().count() > 1
        {
            out = out.replace(&home, "~");
        }
    }
    let rules: [(&LazyLock<Option<Regex>>, &str); 6] = [
        (&PAIRING, "[pairing-url]"),
        (&SIGN_IN, "[sign-in-url]"),
        (&BEARER, "Bearer [redacted]"),
        (&BASIC, "Authorization: Basic [redacted]"),
        (&API_KEY, "x-api-key: [redacted]"),
        (&SECRET, "[redacted]"),
    ];
    for (pattern, replacement) in rules {
        if let Some(pattern) = pattern.as_ref() {
            out = pattern.replace_all(&out, replacement).into_owned();
        }
    }
    out.trim().to_owned()
}

/// What the agent last said on stderr, cleaned. Waits a moment for the last lines when nothing has arrived
/// yet; doesn't wait when something already did.
pub(super) async fn excerpt(conn: &Conn) -> String {
    if conn.stderr_tail().is_empty() {
        let mut done = conn.stderr_done.subscribe();
        let drained = *done.borrow_and_update();
        if !drained {
            let _ = tokio::time::timeout(DRAIN_WAIT, done.changed()).await;
        }
    }
    sanitize(&conn.stderr_tail())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_stderr_is_cleaned_before_it_is_shown() {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let said = format!(
            "error: cannot write {home}/notes.md\0\n\
             open https://claude.ai/pair#abc123\n\
             sign in at https://accounts.google.com/o/oauth2?x=1\n\
             Authorization: Bearer abc.def.ghi\n\
             Authorization: Basic dXNlcjpwYXNz\n\
             x-api-key: sk-live-123\n\
             tokens ghp_abcdef123456 and sk-1234567890 and xoxb-1234-abcd"
        );
        let clean = sanitize(&said);
        assert!(!clean.contains('\0'), "{clean}");
        assert!(
            !clean.contains("abc123"),
            "a pairing link is not repeated: {clean}"
        );
        assert!(
            !clean.contains("accounts.google.com"),
            "a sign-in link is not repeated: {clean}"
        );
        assert!(
            !clean.contains("abc.def.ghi") && !clean.contains("dXNlcjpwYXNz"),
            "{clean}"
        );
        assert!(
            !clean.contains("ghp_") && !clean.contains("xoxb-") && !clean.contains("1234567890"),
            "{clean}"
        );
        assert!(
            clean.contains("[pairing-url]") && clean.contains("[sign-in-url]"),
            "{clean}"
        );
        assert!(
            clean.contains("Bearer [redacted]")
                && clean.contains("Authorization: Basic [redacted]"),
            "{clean}"
        );
        assert!(clean.contains("x-api-key: [redacted]"), "{clean}");
        if home.chars().count() > 1 {
            assert!(
                !clean.contains(&home) && clean.contains("~/notes.md"),
                "a home directory is not repeated: {clean}"
            );
        }
    }
}
