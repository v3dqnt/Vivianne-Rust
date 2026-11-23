//! The canonical URI of a session and its mention in prompt text: `@[label](viv-session:<base64url of the id as JSON>)`.
//! The URI is lossless for any id and has exactly one spelling, so a mention that was edited by hand is refused, not guessed.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::SessionReferenceError;

pub const SCHEME: &str = "viv-session:";

/// A session chosen by the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub session_id: String,
    pub label: String,
}

#[must_use]
pub fn encode_uri(session_id: &str) -> String {
    let json = serde_json::to_string(session_id).unwrap_or_default();
    format!("{SCHEME}{}", URL_SAFE_NO_PAD.encode(json))
}

fn invalid(uri: &str) -> SessionReferenceError {
    SessionReferenceError::invalid(format!("invalid session reference URI {uri:?}"))
}

/// The session id in a canonical URI. Anything that is not exactly what `encode_uri` makes is refused.
pub fn decode_uri(uri: &str) -> Result<String, SessionReferenceError> {
    let payload = uri.strip_prefix(SCHEME).ok_or_else(|| invalid(uri))?;
    if payload.is_empty()
        || !payload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid(uri));
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid(uri))?;
    let id: String = serde_json::from_slice(&bytes).map_err(|_| invalid(uri))?;
    if encode_uri(&id) != uri {
        return Err(invalid(uri));
    }
    Ok(id)
}

fn escape_label(label: &str) -> String {
    label
        .chars()
        .flat_map(|c| {
            if c == '\\' || c == ']' {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

fn unescape_label(label: &str) -> String {
    let mut out = String::new();
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// `@[label](uri)`, with the label escaped.
#[must_use]
pub fn format_mention(session_id: &str, label: Option<&str>) -> String {
    format!(
        "@[{}]({})",
        escape_label(label.unwrap_or(session_id)),
        encode_uri(session_id)
    )
}

/// Text with its mentions made readable, and the references found, in order of appearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The text with each mention replaced by `@label`.
    pub text: String,
    pub references: Vec<Reference>,
}

/// The end of a Markdown mention starting at `at` (`@[`): the label, the URI, and the byte after the closing paren.
fn markdown_mention(text: &str, at: usize) -> Option<(String, &str, usize)> {
    let rest = &text[at + 2..];
    let mut label_end = None;
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            ']' => {
                label_end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let label_end = label_end?;
    let after = &rest[label_end + 1..];
    let uri_part = after.strip_prefix('(')?;
    let uri_end = uri_part.find(|c: char| c.is_whitespace() || c == ')')?;
    if !uri_part[uri_end..].starts_with(')') || !uri_part.starts_with(SCHEME) {
        return None;
    }
    let consumed = at + 2 + label_end + 1 + 1 + uri_end + 1;
    Some((
        unescape_label(&rest[..label_end]),
        &uri_part[..uri_end],
        consumed,
    ))
}

/// Extracts Markdown mentions and bare canonical URIs. A Markdown mention fails on any malformed URI; bare text counts as a
/// reference only when it has a non-empty base64url payload, and then fails if it is not canonical.
pub fn parse_text(text: &str) -> Result<Parsed, SessionReferenceError> {
    let mut out = String::new();
    let mut references = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("@[")
            && let Some((label, uri, next)) = markdown_mention(text, i)
        {
            let id = decode_uri(uri)?;
            out.push('@');
            out.push_str(&label);
            references.push(Reference {
                session_id: id,
                label,
            });
            i = next;
            continue;
        }
        if let Some(after_scheme) = rest.strip_prefix(SCHEME) {
            let payload_len = after_scheme
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
                .count();
            if payload_len > 0 {
                let uri = &rest[..SCHEME.len() + payload_len];
                let id = decode_uri(uri)?;
                out.push('@');
                out.push_str(&id);
                references.push(Reference {
                    label: id.clone(),
                    session_id: id,
                });
                i += uri.len();
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    Ok(Parsed {
        text: out,
        references,
    })
}
