//! What an event may carry. Event `props` used to be free-form, so a caller could put a prompt, a file path or a
//! key in them and have it written to disk (and, with sharing on, sent). Now anything that is not a short
//! identifier, a number or a yes/no is dropped before it is recorded.

use serde_json::{Map, Value};

/// Most properties one event keeps.
const MAX_PROPS: usize = 8;
/// Most bytes the properties take once written, so one event is always a short line.
pub(crate) const MAX_PROPS_BYTES: usize = 640;
/// Longest property name.
const MAX_NAME: usize = 32;
/// Longest text value: enough for a model id or a tool name, too short for a sentence.
const MAX_TEXT: usize = 48;
/// Longest event kind.
const MAX_KIND: usize = 48;
/// A run of letters and digits this long looks like a key or a token, not a name.
const KEY_LIKE_RUN: usize = 24;

/// `kind` if it is a dotted lower-case name like `turn.complete`.
pub(crate) fn clean_kind(kind: &str) -> Option<&str> {
    let ok = !kind.is_empty()
        && kind.len() <= MAX_KIND
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_');
    ok.then_some(kind)
}

/// The properties that may be recorded: an object whose values are numbers, booleans or short identifiers.
/// Everything else (nested data, nulls, long or free text, anything that looks like a path or a key) is dropped.
pub(crate) fn clean(props: &Value) -> Value {
    let mut kept = Map::new();
    if let Value::Object(given) = props {
        for (name, value) in given.iter().take(MAX_PROPS * 2) {
            if kept.len() == MAX_PROPS {
                break;
            }
            if is_name(name) && is_allowed(value) {
                kept.insert(name.clone(), value.clone());
            }
        }
    }
    while serde_json::to_string(&kept).is_ok_and(|text| text.len() > MAX_PROPS_BYTES) {
        let Some(last) = kept.keys().next_back().cloned() else {
            break;
        };
        kept.remove(&last);
    }
    Value::Object(kept)
}

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn is_allowed(value: &Value) -> bool {
    match value {
        Value::Bool(_) | Value::Number(_) => true,
        Value::String(text) => is_identifier(text),
        Value::Null | Value::Array(_) | Value::Object(_) => false,
    }
}

/// A short name such as `claude-sonnet-5`, `openai/gpt-5.1`, `read_file` or `agent:claude-code`: no spaces, no
/// path or URL shape, no long run of characters like a key.
fn is_identifier(text: &str) -> bool {
    let chars_ok = text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'+' | b'/'));
    let shaped_like_a_place = text.starts_with(['/', '.', '~'])
        || text.contains("..")
        || text.contains("//")
        || text.contains(":/");
    !text.is_empty()
        && text.len() <= MAX_TEXT
        && chars_ok
        && !shaped_like_a_place
        && !has_key_like_run(text)
}

fn has_key_like_run(text: &str) -> bool {
    let mut run = 0;
    for byte in text.bytes() {
        run = if byte.is_ascii_alphanumeric() {
            run + 1
        } else {
            0
        };
        if run >= KEY_LIKE_RUN {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn numbers_booleans_and_short_identifiers_are_kept() {
        let props = json!({ "provider": "openai", "model": "gpt-5.1", "ms": 1234, "ok": true, "tools": 3, "id": "agent:claude-code", "via": "nvidia/llama-3.1-70b" });
        assert_eq!(clean(&props), props);
    }

    #[test]
    fn prompt_text_paths_urls_keys_and_nested_data_are_dropped() {
        let props = json!({
            "prompt": "please fix the bug in my login code",
            "path": "C:/Users/me/secret.txt",
            "unix": "/home/me/secret.txt",
            "backslash": "C:\\Users\\me",
            "url": "https://example.com/x",
            "key": "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
            "hex": "0123456789abcdef0123456789abcdef",
            "nested": { "a": 1 },
            "list": [1, 2],
            "nothing": null,
            "long": "x".repeat(60),
            "bad name": 1,
            "ok": true,
        });
        assert_eq!(clean(&props), json!({ "ok": true }));
    }

    #[test]
    fn only_an_object_can_carry_properties() {
        assert_eq!(clean(&json!("text")), json!({}));
        assert_eq!(clean(&Value::Null), json!({}));
    }

    #[test]
    fn the_count_and_the_size_are_capped() {
        let many: Map<String, Value> = (0..30).map(|n| (format!("k{n:02}"), json!(n))).collect();
        let cleaned = clean(&Value::Object(many));
        assert_eq!(cleaned.as_object().map(Map::len), Some(MAX_PROPS));
        let wide: Map<String, Value> = (0..MAX_PROPS)
            .map(|n| {
                (
                    format!("{}{n}", "n".repeat(30)),
                    json!("v".repeat(MAX_TEXT)),
                )
            })
            .collect();
        let text = serde_json::to_string(&clean(&Value::Object(wide))).unwrap();
        assert!(text.len() <= MAX_PROPS_BYTES);
    }

    #[test]
    fn a_kind_is_a_short_dotted_lower_case_name() {
        assert_eq!(clean_kind("turn.complete"), Some("turn.complete"));
        assert_eq!(clean_kind("agent.sign_in"), Some("agent.sign_in"));
        assert_eq!(clean_kind("Turn Complete"), None);
        assert_eq!(clean_kind(""), None);
        assert_eq!(clean_kind(&"a".repeat(60)), None);
    }
}
