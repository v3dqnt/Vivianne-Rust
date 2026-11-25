//! A conversation as a session log file: one JSON object per line, in a fixed shape other tools can read, with secrets
//! taken out. Port of the part of the original `session-log-deepseek` that is not a network upload: the canonical log, its
//! exclusions, and sending only the part after a watermark. Nothing here sends anything anywhere; a file is the only way
//! out, and the person decides where it goes.
//!
//! Line 1 is the header (`type: session`). Each later line is one message (`type: message`) with its position `seq`. With
//! `after`, the lines for messages before that position are left out, so a second export continues the first.

use std::io::Write;

use serde_json::{Value, json};

use crate::store::SavedThread;

/// The name and version of the shape written, so a reader knows what it has.
pub const FORMAT: &str = "vivianne-session-log";
pub const VERSION: u32 = 1;

/// What goes into an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Take secrets and the home folder's name out of every text. On unless the person turns it off.
    pub redact: bool,
    /// Skip the messages before this position (the watermark of an earlier export).
    pub after: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            redact: true,
            after: 0,
        }
    }
}

const HIDDEN: &str = "[REDACTED]";

/// Starts of tokens that are credentials, with the shortest length one of them has.
const SECRET_PREFIXES: [&str; 11] = [
    "sk-",
    "sk_",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "AIza",
    "eyJ",
];
/// Words that name a value as secret in `name=value` and `name: value`.
const SECRET_NAMES: [&str; 7] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "authorization",
];

fn is_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | ',' | ';' | '(' | ')' | '<' | '>' | '[' | ']' | '{' | '}' | '`'
        )
}

fn looks_secret(token: &str) -> bool {
    token.len() >= 20 && SECRET_PREFIXES.iter().any(|p| token.starts_with(p))
}

/// Takes secrets out of `text`: tokens that start like a known credential, the value after `Bearer`, the value in
/// `password=...` and the like, private key blocks, and the home folder's path (which says whose machine it was).
#[must_use]
pub fn redact(text: &str) -> String {
    let mut text = text.to_owned();
    while let Some(start) = text.find("-----BEGIN ") {
        let Some(end) = text[start..]
            .find("PRIVATE KEY-----")
            .map(|e| start + e + "PRIVATE KEY-----".len())
        else {
            break;
        };
        // Past the closing line when there is one, so a whole block goes.
        let end = text[end..]
            .find("-----END ")
            .and_then(|e| {
                text[end + e..]
                    .find("-----\n")
                    .or_else(|| text[end + e..].rfind("-----"))
                    .map(|c| end + e + c + 5)
            })
            .unwrap_or(end);
        text.replace_range(start..end.min(text.len()), HIDDEN);
    }
    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .and_then(|h| h.into_string().ok())
        .filter(|h| h.len() > 3)
    {
        text = text
            .replace(&home, "~")
            .replace(&home.replace('\\', "/"), "~");
    }
    let mut out = String::with_capacity(text.len());
    let mut after_bearer = false;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let split = rest.find(|c| !is_separator(c)).unwrap_or(rest.len());
        out.push_str(&rest[..split]);
        rest = &rest[split..];
        let end = rest.find(is_separator).unwrap_or(rest.len());
        let token = &rest[..end];
        rest = &rest[end..];
        if token.is_empty() {
            continue;
        }
        let named = token.find(['=', ':']).filter(|&i| {
            i + 1 < token.len()
                && SECRET_NAMES
                    .iter()
                    .any(|n| token[..i].to_lowercase().contains(n))
        });
        if after_bearer || looks_secret(token) {
            out.push_str(HIDDEN);
        } else if let Some(i) = named {
            out.push_str(&token[..=i]);
            out.push_str(HIDDEN);
        } else {
            out.push_str(token);
        }
        after_bearer = token.eq_ignore_ascii_case("bearer");
    }
    out
}

fn text_of(text: &str, options: Options) -> String {
    if options.redact {
        redact(text)
    } else {
        text.to_owned()
    }
}

/// The lines of the log for `thread`: the header (unless `after` skips into the middle), then one per message.
#[must_use]
pub fn lines(thread: &SavedThread, options: Options) -> Vec<Value> {
    let mut out = Vec::new();
    if options.after == 0 {
        out.push(json!({
            "type": "session",
            "format": FORMAT,
            "version": VERSION,
            "id": thread.id,
            "title": text_of(&thread.title, options),
            "workspace": text_of(&thread.workspace, options),
            "provider": thread.provider,
            "model": thread.model,
            "updated_at": thread.updated_at,
            "messages": thread.history.len(),
            "redacted": options.redact,
        }));
    }
    for (seq, message) in thread.history.iter().enumerate().skip(options.after) {
        let calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|c| {
                let args = serde_json::from_str(&text_of(&c.args.to_string(), options))
                    .unwrap_or_else(|_| Value::String(HIDDEN.to_owned()));
                json!({ "id": c.id, "name": c.name, "args": args })
            })
            .collect();
        let mut object = serde_json::Map::new();
        object.insert("type".into(), Value::from("message"));
        object.insert("seq".into(), Value::from(seq));
        object.insert("role".into(), json!(message.role));
        object.insert("text".into(), Value::from(text_of(&message.text, options)));
        if !calls.is_empty() {
            object.insert("tool_calls".into(), Value::Array(calls));
        }
        if let Some(id) = &message.tool_call_id {
            object.insert("tool_call_id".into(), Value::from(id.clone()));
        }
        if let Some(at) = message.at {
            object.insert("at".into(), Value::from(at));
        }
        if let Some(timing) = message.timing {
            object.insert("timing".into(), json!(timing));
        }
        // Attachments are named by count only: what they hold is not part of the log.
        if !message.attachments.is_empty() {
            object.insert("attachments".into(), Value::from(message.attachments.len()));
        }
        out.push(Value::Object(object));
    }
    out
}

/// Writes the log for `thread` to `out`, one object per line. Returns how many lines were written.
pub fn write_to(
    out: &mut impl Write,
    thread: &SavedThread,
    options: Options,
) -> Result<usize, String> {
    let all = lines(thread, options);
    for line in &all {
        writeln!(out, "{line}").map_err(|e| e.to_string())?;
    }
    Ok(all.len())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::{Message, Mode, Role, ToolCall};

    use super::*;

    fn thread() -> SavedThread {
        SavedThread {
            id: "t".into(),
            workspace: "/w".into(),
            title: "Fix it".into(),
            title_source: "auto".into(),
            provider: "p".into(),
            model: "m".into(),
            mode: Mode::Ask,
            updated_at: 5,
            history: vec![
                Message::new(
                    Role::User,
                    "deploy with key sk-abcdefghijklmnopqrstuvwx please",
                )
                .at(10),
                Message::assistant(
                    "running".to_owned(),
                    vec![ToolCall {
                        id: "c".into(),
                        name: "shell".into(),
                        args: json!({ "command": "curl -H 'Authorization: Bearer abc123' x" }),
                    }],
                ),
                Message::tool_result("c", "password=hunter2 ok".to_owned()),
            ],
        }
    }

    #[test]
    fn the_header_comes_first_then_one_line_per_message_with_its_position() {
        let got = lines(&thread(), Options::default());
        assert_eq!(got.len(), 4);
        assert_eq!(
            (
                got[0]["type"].as_str(),
                got[0]["format"].as_str(),
                got[0]["messages"].as_u64()
            ),
            (Some("session"), Some(FORMAT), Some(3))
        );
        assert_eq!(
            (
                got[1]["seq"].as_u64(),
                got[1]["role"].as_str(),
                got[1]["at"].as_u64()
            ),
            (Some(0), Some("user"), Some(10))
        );
        assert_eq!(got[2]["tool_calls"][0]["name"], "shell");
        assert_eq!(got[3]["tool_call_id"], "c");
    }

    #[test]
    fn credentials_are_taken_out_of_text_and_of_tool_arguments_and_the_rest_is_kept() {
        let got = lines(&thread(), Options::default());
        assert_eq!(got[1]["text"], format!("deploy with key {HIDDEN} please"));
        let command = got[2]["tool_calls"][0]["args"]["command"]
            .as_str()
            .expect("a command");
        assert!(
            command.contains("Bearer [REDACTED]") && !command.contains("abc123"),
            "{command}"
        );
        assert_eq!(got[3]["text"], format!("password={HIDDEN} ok"));
        let raw = lines(
            &thread(),
            Options {
                redact: false,
                after: 0,
            },
        );
        assert!(
            raw[1]["text"]
                .as_str()
                .unwrap()
                .contains("sk-abcdefghijklmnopqrstuvwx")
                && raw[0]["redacted"] == false
        );
    }

    #[test]
    fn a_private_key_block_goes_whole_and_ordinary_words_are_left_alone() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----\nafter sk-short eyJ";
        let got = redact(text);
        assert!(
            !got.contains("MIIabc") && got.contains("before") && got.contains("after"),
            "{got}"
        );
        assert_eq!(
            redact("the token budget is 5 and a sk-short"),
            "the token budget is 5 and a sk-short"
        );
    }

    #[test]
    fn a_later_export_can_continue_from_where_an_earlier_one_stopped() {
        let rest = lines(
            &thread(),
            Options {
                redact: true,
                after: 2,
            },
        );
        assert_eq!(
            rest.len(),
            1,
            "no header, only the messages after the watermark"
        );
        assert_eq!(rest[0]["seq"], 2);
        let mut bytes = Vec::new();
        assert_eq!(write_to(&mut bytes, &thread(), Options::default()), Ok(4));
        assert_eq!(String::from_utf8(bytes).unwrap().lines().count(), 4);
    }
}
