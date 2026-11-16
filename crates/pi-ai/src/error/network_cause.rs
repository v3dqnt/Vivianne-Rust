//! Port of `src/error/network-cause.ts`.
//!
//! Local transport diagnosis for a rejected `fetch`.
//!
//! Bun and Node describe the same outage with different shapes: Bun throws a `TypeError` carrying
//! `code: "ETIMEOUT" | "FailedToOpenSocket" | "ConnectionRefused"`, undici nests an
//! `ENOTFOUND`/`ECONNREFUSED` errno under `TypeError: fetch failed` (sometimes through an
//! `AggregateError`), and `fetchWithRetry` may already have flattened the chain into a
//! message-only `Error`. Both the code and the text are inspected so every one of those shapes
//! lands in the right bucket.

use std::collections::VecDeque;
use std::sync::OnceLock;

use regex::Regex;

use super::flags::ErrorLike;

/// Transport-level failure families that get their own user-visible wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkFailureKind {
    Offline,
    Unreachable,
    Refused,
    Tls,
}

/// What [`classify_network_cause`] reports about a rejected `fetch`: the failure family plus the
/// bits its sentence needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkFailure {
    pub kind: NetworkFailureKind,
    /// Runtime error code when the chain exposed one (`ETIMEOUT`, `FailedToOpenSocket`, ...).
    pub code: Option<String>,
    /// Short parenthetical detail for the user-visible message.
    pub detail: Option<String>,
    /// Host named by the error text, when it named one.
    pub host: Option<String>,
}

const MAX_CAUSE_DEPTH: usize = 8;
const MAX_AGGREGATE_BRANCH: usize = 4;
const MAX_LINKS: usize = 16;
const MAX_DETAIL_LENGTH: usize = 120;
const UNKNOWN_HOST: &str = "the API host";

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static C: OnceLock<Regex> = OnceLock::new();
            C.get_or_init(|| Regex::new($pat).expect("static pattern"))
        }
    };
}

lazy_re!(offline_code, r"(?i)^(?:ETIMEOUT|EAI_AGAIN|ENETUNREACH|EHOSTUNREACH)$");
lazy_re!(offline_text, r"(?i)\b(?:ETIMEOUT|EAI_AGAIN|ENETUNREACH|EHOSTUNREACH)\b");
lazy_re!(unreachable_code, r"(?i)^(?:ENOTFOUND|FailedToOpenSocket)$");
lazy_re!(unreachable_text, r"(?i)\b(?:ENOTFOUND|FailedToOpenSocket|getaddrinfo)\b");
lazy_re!(refused_code, r"(?i)^(?:ECONNREFUSED|ConnectionRefused)$");
lazy_re!(refused_text, r"(?i)\bECONNREFUSED\b|\bConnectionRefused\b|\bUnable to connect\b");
lazy_re!(
    tls_code,
    r"(?i)^(?:CERT_|SELF_SIGNED|UNABLE_TO_GET_ISSUER_CERT|UNABLE_TO_VERIFY|DEPTH_ZERO_SELF_SIGNED_CERT|ERR_TLS_CERT)"
);
lazy_re!(
    tls_text,
    r"(?i)unable to verify|certificate verif(?:y|ication)|self[- ]signed certificate|\bCERT_[A-Z_]+\b|\bSELF_SIGNED[A-Z_]*\b|\bUNABLE_TO_GET_ISSUER_CERT[A-Z_]*\b|\bUNABLE_TO_VERIFY_LEAF_SIGNATURE\b"
);
lazy_re!(getaddrinfo_host, r"(?i)\bgetaddrinfo\s+[A-Z_]+\s+([^\s,)]+)");
// The TS pattern ends in a lookahead `(?=[\s,)]|$)`; written as a consuming group here, which
// leaves the capture group unchanged.
lazy_re!(connect_host, r"(?i)\b(?:connect|connection to)\s+[A-Z_]+\s+([^\s,)]+?)(?::\d+)?(?:[\s,)]|$)");
lazy_re!(bare_host, r"(?i)\b((?:[a-z0-9-]+\.)+[a-z]{2,})\b");
lazy_re!(getaddrinfo_detail, r"(?i)\bgetaddrinfo\s+[A-Z_]+");
lazy_re!(errno_detail, r"(?i)\b(?:ETIMEOUT|ENOTFOUND|EAI_AGAIN|ENETUNREACH|EHOSTUNREACH|FailedToOpenSocket)\b");

fn kind_of(code: Option<&str>, message: Option<&str>) -> Option<NetworkFailureKind> {
    use NetworkFailureKind as K;
    let code_is = |re: &Regex| code.map_or(false, |c| re.is_match(c));
    let text_is = |re: &Regex| message.map_or(false, |m| re.is_match(m));
    // TLS first: a certificate failure usually also reads as a failed connect.
    if code_is(tls_code()) || text_is(tls_text()) {
        return Some(K::Tls);
    }
    if code_is(refused_code()) || text_is(refused_text()) {
        return Some(K::Refused);
    }
    // Offline before unreachable: `getaddrinfo ETIMEOUT` matches both text patterns.
    if code_is(offline_code()) || text_is(offline_text()) {
        return Some(K::Offline);
    }
    if code_is(unreachable_code()) || text_is(unreachable_text()) {
        return Some(K::Unreachable);
    }
    None
}

/// First host named by any message seen so far, most specific pattern first.
fn host_from_texts(texts: &[String]) -> Option<String> {
    for pattern in [getaddrinfo_host(), connect_host(), bare_host()] {
        for text in texts {
            if let Some(host) = pattern.captures(text).and_then(|c| c.get(1)) {
                if !host.as_str().is_empty() {
                    return Some(host.as_str().to_owned());
                }
            }
        }
    }
    None
}

fn detail_of(kind: NetworkFailureKind, code: Option<&str>, message: Option<&str>) -> Option<String> {
    // The refused template already carries its own wording.
    if kind == NetworkFailureKind::Refused {
        return None;
    }
    if kind == NetworkFailureKind::Tls {
        if let Some(c) = code {
            return Some(c.to_owned());
        }
    }
    let pattern = if kind == NetworkFailureKind::Tls { tls_text() } else { getaddrinfo_detail() };
    if let Some(matched) = message.and_then(|m| pattern.find(m)).map(|m| m.as_str()) {
        static WS: OnceLock<Regex> = OnceLock::new();
        let ws = WS.get_or_init(|| Regex::new(r"\s+").unwrap());
        let squeezed = ws.replace_all(matched, " ");
        return Some(squeezed.trim().chars().take(MAX_DETAIL_LENGTH).collect());
    }
    if kind == NetworkFailureKind::Tls {
        return None;
    }
    // A DNS/route failure without a `getaddrinfo ...` fragment still names its errno; keeping it
    // in the sentence is what keeps the rendered message matching the transient-transport pattern.
    code.map(str::to_owned)
        .or_else(|| message.and_then(|m| errno_detail().find(m)).map(|m| m.as_str().to_owned()))
}

/// Walk an error's `cause`/`errors` graph (bounded depth) and report the outermost link that
/// describes a local transport failure. Returns `None` for anything else, including HTTP-level
/// provider errors.
pub fn classify_network_cause(err: &dyn ErrorLike) -> Option<NetworkFailure> {
    struct Link<'a> {
        value: &'a dyn ErrorLike,
        depth: usize,
    }
    let mut queue: VecDeque<Link<'_>> = VecDeque::new();
    queue.push_back(Link { value: err, depth: 0 });
    let mut texts: Vec<String> = Vec::new();
    let mut links = 0;

    while links < MAX_LINKS {
        let Some(link) = queue.pop_front() else { break };
        links += 1;
        let Link { value, depth } = link;

        let code = value.code();
        let message = value.message();
        if let Some(m) = message {
            texts.push(m.to_owned());
        }

        if let Some(kind) = kind_of(code, message) {
            return Some(NetworkFailure {
                kind,
                code: code.map(str::to_owned),
                detail: detail_of(kind, code, message),
                host: host_from_texts(&texts),
            });
        }

        if depth >= MAX_CAUSE_DEPTH {
            continue;
        }
        if let Some(cause) = value.cause() {
            queue.push_back(Link { value: cause, depth: depth + 1 });
        }
        for nested in value.errors().into_iter().take(MAX_AGGREGATE_BRANCH) {
            queue.push_back(Link { value: nested, depth: depth + 1 });
        }
    }
    None
}

/// User-visible sentence for a network failure, or `None` when the error is not one.
/// `request_url` wins over any host parsed out of the error text.
pub fn network_failure_message(err: &dyn ErrorLike, request_url: Option<&str>) -> Option<String> {
    let failure = classify_network_cause(err)?;
    let url_host = request_url.filter(|u| !u.is_empty()).and_then(|u| {
        let parsed = url::Url::parse(u).ok()?;
        let host = parsed.host_str()?;
        Some(match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        })
    });
    let host = url_host.or(failure.host.clone()).unwrap_or_else(|| UNKNOWN_HOST.to_owned());
    let detail = failure.detail.as_ref().map_or(String::new(), |d| format!(" ({d})"));
    Some(match failure.kind {
        NetworkFailureKind::Offline => format!(
            "No internet connection: couldn't reach {host}{detail}. Check your network or proxy and try again."
        ),
        NetworkFailureKind::Unreachable => {
            format!("Couldn't reach {host}{detail}. Check the URL, your network, or proxy.")
        }
        NetworkFailureKind::Refused => format!(
            "Couldn't connect to {host} (connection refused). Check that the server is running and reachable."
        ),
        NetworkFailureKind::Tls => format!(
            "TLS certificate verification failed for {host}{detail}. If you're behind a TLS-inspecting proxy, its CA may need to be trusted."
        ),
    })
}

/// A rejected `fetch` re-described with [`network_failure_message`], keeping the original error as
/// the cause.
#[derive(Debug)]
pub struct NetworkFailureError {
    pub message: String,
    pub cause: Box<dyn std::error::Error + Send + Sync>,
}

impl std::fmt::Display for NetworkFailureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for NetworkFailureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Re-describe a rejected `fetch` with [`network_failure_message`], keeping the original error as
/// `cause`. Anything that is not a local transport failure comes back as `Err(cause)` untouched, so
/// provider/HTTP errors keep their identity.
pub fn network_failure_error(
    err: &dyn ErrorLike,
    cause: Box<dyn std::error::Error + Send + Sync>,
    request_url: Option<&str>,
) -> Result<NetworkFailureError, Box<dyn std::error::Error + Send + Sync>> {
    match network_failure_message(err, request_url) {
        Some(message) => Ok(NetworkFailureError { message, cause }),
        None => Err(cause),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::TextError;

    struct Node<'a> {
        code: Option<&'a str>,
        message: &'a str,
        cause: Option<Box<Node<'a>>>,
        errors: Vec<Node<'a>>,
    }

    impl ErrorLike for Node<'_> {
        fn code(&self) -> Option<&str> {
            self.code
        }
        fn message(&self) -> Option<&str> {
            Some(self.message)
        }
        fn cause(&self) -> Option<&dyn ErrorLike> {
            self.cause.as_deref().map(|c| c as &dyn ErrorLike)
        }
        fn errors(&self) -> Vec<&dyn ErrorLike> {
            self.errors.iter().map(|e| e as &dyn ErrorLike).collect()
        }
    }

    fn node<'a>(code: Option<&'a str>, message: &'a str) -> Node<'a> {
        Node { code, message, cause: None, errors: Vec::new() }
    }

    #[test]
    fn dns_failure_is_unreachable_and_names_the_host() {
        let mut outer = node(None, "fetch failed");
        outer.cause = Some(Box::new(node(Some("ENOTFOUND"), "getaddrinfo ENOTFOUND api.example.com")));
        let f = classify_network_cause(&outer).unwrap();
        assert_eq!(f.kind, NetworkFailureKind::Unreachable);
        assert_eq!(f.host.as_deref(), Some("api.example.com"));
        assert_eq!(
            network_failure_message(&outer, None).unwrap(),
            "Couldn't reach api.example.com (getaddrinfo ENOTFOUND). Check the URL, your network, or proxy."
        );
    }

    #[test]
    fn timeout_is_offline_and_url_host_wins() {
        let e = node(Some("ETIMEOUT"), "request failed");
        let msg = network_failure_message(&e, Some("https://api.openai.com:8443/v1")).unwrap();
        assert_eq!(
            msg,
            "No internet connection: couldn't reach api.openai.com:8443 (ETIMEOUT). Check your network or proxy and try again."
        );
    }

    #[test]
    fn refused_and_tls_and_aggregate() {
        let r = node(Some("ECONNREFUSED"), "connect ECONNREFUSED 127.0.0.1:11434");
        let msg = network_failure_message(&r, None).unwrap();
        assert!(msg.starts_with("Couldn't connect to 127.0.0.1 (connection refused)"));
        let tls = node(Some("SELF_SIGNED_CERT_IN_CHAIN"), "self signed certificate in certificate chain");
        let f = classify_network_cause(&tls).unwrap();
        assert_eq!((f.kind, f.detail.as_deref()), (NetworkFailureKind::Tls, Some("SELF_SIGNED_CERT_IN_CHAIN")));
        let mut agg = node(None, "AggregateError");
        agg.errors = vec![node(None, "nothing"), node(Some("ECONNREFUSED"), "connect ECONNREFUSED ::1:80")];
        assert_eq!(classify_network_cause(&agg).unwrap().kind, NetworkFailureKind::Refused);
    }

    #[test]
    fn non_network_errors_are_ignored_and_strings_work() {
        assert!(classify_network_cause(&node(None, "HTTP 500 server error")).is_none());
        assert_eq!(
            classify_network_cause(&TextError("getaddrinfo EAI_AGAIN foo.bar.io")).unwrap().kind,
            NetworkFailureKind::Offline
        );
        let cause: Box<dyn std::error::Error + Send + Sync> = "boom".into();
        assert!(network_failure_error(&node(None, "ok"), cause, None).is_err());
    }
}
