//! Fitting another conversation into a byte budget. Only what the two people said survives: typed messages, replies, and
//! earlier condensed checkpoints, never tool output, reasoning or what the app injected. When it is too big, the oldest
//! messages go whole (the newest and the checkpoints stay), then the longest message is cut in the middle with a notice.

use serde::Serialize;
use vivianne_core::{Message, Role};

/// Whether a message is a condensed checkpoint of earlier history.
#[must_use]
pub fn is_checkpoint(message: &Message) -> bool {
    message.text.contains("<compacted_history>")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub role: &'static str,
    pub text: String,
}

#[derive(Debug, Clone)]
struct Projected {
    item: Item,
    checkpoint: bool,
    original: String,
    omitted_bytes: usize,
}

/// The data quoted into the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionData {
    pub session_id: String,
    pub label: String,
    pub cwd: Option<String>,
    pub captured_messages: usize,
    pub conversation: Vec<Item>,
}

/// What was kept and what was not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub compacted: bool,
    pub original_messages: usize,
    pub retained_messages: usize,
    pub omitted_messages: usize,
    pub omitted_bytes: usize,
    pub truncated: bool,
}

/// The preview that fits, the full projection, and the statistics.
#[derive(Debug, Clone)]
pub struct Retained {
    pub data: SessionData,
    pub full: SessionData,
    pub stats: Stats,
}

/// JSON that cannot spell an XML-like opening tag: the parse result is unchanged and no `<` appears in the text.
#[must_use]
pub fn tag_safe_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_default()
        .replace('<', "\\u003c")
}

fn project(messages: &[Message]) -> Vec<Projected> {
    messages
        .iter()
        .filter_map(|m| {
            let (role, checkpoint) = match m.role {
                Role::User if m.is_typed() => ("user", is_checkpoint(m)),
                Role::Assistant if !m.text.trim().is_empty() => ("assistant", false),
                _ => return None,
            };
            (!m.text.is_empty()).then(|| Projected {
                item: Item {
                    role,
                    text: m.text.clone(),
                },
                checkpoint,
                original: m.text.clone(),
                omitted_bytes: 0,
            })
        })
        .collect()
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `text` with its middle removed to fit `max_output_bytes`, and a notice of how much was removed.
fn truncate_with_notice(text: &str, max_output_bytes: usize) -> (String, usize) {
    if text.len() <= max_output_bytes {
        return (text.to_owned(), 0);
    }
    let (mut low, mut high) = (0usize, max_output_bytes);
    let mut best = (String::new(), text.len());
    while low <= high {
        let retained = (low + high) / 2;
        let head = floor_boundary(text, retained.div_ceil(2));
        let tail_start = {
            let mut at = text.len().saturating_sub(retained / 2);
            while at < text.len() && !text.is_char_boundary(at) {
                at += 1;
            }
            at.max(head)
        };
        let omitted = tail_start - head;
        let candidate = format!(
            "{}{}\n[… omitted {omitted} UTF-8 bytes …]",
            &text[..head],
            &text[tail_start..]
        );
        if candidate.len() <= max_output_bytes {
            best = (candidate, omitted);
            low = retained + 1;
        } else if retained == 0 {
            break;
        } else {
            high = retained - 1;
        }
    }
    best
}

/// Fits one conversation into a rendered JSON byte cap. `None` when what cannot be cut (the fixed data) does not fit.
#[must_use]
pub fn retain(
    session_id: &str,
    label: &str,
    cwd: Option<&str>,
    messages: &[Message],
    max_bytes: usize,
) -> Option<Retained> {
    let original = project(messages);
    let mut retained = original.clone();
    let (mut omitted_messages, mut dropped_bytes) = (0usize, 0usize);
    let data = |retained: &[Projected]| SessionData {
        session_id: session_id.to_owned(),
        label: label.to_owned(),
        cwd: cwd.map(str::to_owned),
        captured_messages: messages.len(),
        conversation: retained.iter().map(|p| p.item.clone()).collect(),
    };
    let full = data(&retained);
    let size = |retained: &[Projected]| tag_safe_json(&data(retained)).len();
    while size(&retained) > max_bytes {
        let newest = retained.len().saturating_sub(1);
        let Some(drop_at) = retained
            .iter()
            .enumerate()
            .position(|(i, p)| !p.checkpoint && i != newest)
        else {
            break;
        };
        let removed = retained.remove(drop_at);
        omitted_messages += 1;
        dropped_bytes += removed.original.len();
    }
    while size(&retained) > max_bytes {
        let (longest, longest_bytes) = retained
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.item.text.len()))
            .max_by_key(|(_, n)| *n)?;
        if longest_bytes == 0 {
            return None;
        }
        let overflow = size(&retained) - max_bytes;
        let target = longest_bytes.saturating_sub(overflow);
        let (shortened, omitted) = truncate_with_notice(&retained[longest].original, target);
        if shortened == retained[longest].item.text {
            return None;
        }
        retained[longest].item.text = shortened;
        retained[longest].omitted_bytes = omitted;
    }
    let omitted_bytes = retained.iter().map(|p| p.omitted_bytes).sum::<usize>() + dropped_bytes;
    let stats = Stats {
        compacted: original.iter().any(|p| p.checkpoint),
        original_messages: original.len(),
        retained_messages: retained.len(),
        omitted_messages,
        omitted_bytes,
        truncated: omitted_messages > 0 || omitted_bytes > 0,
    };
    Some(Retained {
        data: data(&retained),
        full,
        stats,
    })
}
