//! Condensing the older part of a long conversation into one checkpoint, so the run can go on after the window
//! fills. The cut, the summary text and the checkpoint are here; asking the model is the agent's job.
//!
//! From the old app's second compaction: the cut never separates a tool call from its result (see
//! [`crate::history::pick_cut`]), the summary asks for fixed sections and merges an earlier checkpoint forward,
//! and a summary that is not smaller than what it replaces is thrown away.

use vivianne_core::{Message, Role};

use crate::context::{estimate_tokens, message_tokens};

pub const OPEN: &str = "<compacted_history>";
const CLOSE: &str = "</compacted_history>";

/// How many characters of a tool result the summariser sees: the first try, then a shorter one.
pub const PREVIEW_CHARS: [usize; 2] = [500, 150];

/// Start condensing when the conversation fills this share of the window.
pub const THRESHOLD: f32 = 0.8;
/// Keep about this share of the window, the newest part, word for word.
pub const KEEP: f32 = 0.2;
/// Too short to be worth condensing.
pub const MIN_MESSAGES: usize = 6;

pub const INSTRUCTION: &str = "You are compacting an AI coding agent's conversation so the agent can carry on from your summary alone: it will not see the original messages. Write one checkpoint in EXACTLY these sections, in this order, each starting with its name in bold. Use terse bullets, and write (none) for an empty section rather than dropping it.

**Goal**: what the user asked for, and how it changed; quote their exact words where the wording matters.
**Constraints and preferences**: instructions, corrections and preferences the user gave, faithfully.
**Progress**: Done, In progress and Blocked items.
**Key decisions**: each with why.
**Tried and failed**: every approach that didn't work, with the exact error or reason, so it isn't tried again.
**Files and code**: exact paths, with why each matters and what changed.
**Current work**: precisely what was happening at the end.
**Next step**: the single next action, in line with the user's latest request, or (none).

Keep exact file paths, commands, error text, identifiers, numbers and signatures. Do not mention this summarisation or that the context was compacted. Do not call tools. If the input already contains a <compacted_history> block it is an EARLIER checkpoint: don't copy it forward. Keep what is still true, drop what is stale, merge in what is newer, and write one consolidated checkpoint in the same sections.";

/// Tokens in a run of messages.
#[must_use]
pub fn tokens_of(messages: &[Message]) -> u32 {
    messages.iter().map(message_tokens).fold(0, u32::saturating_add)
}

/// A summary is only worth inserting if it says something and is smaller than what it replaces.
#[must_use]
pub fn shrinks(summary: &str, replaced_tokens: u32) -> bool {
    !summary.trim().is_empty() && estimate_tokens(summary) < replaced_tokens
}

/// The conversation as text for the summariser: what was said in full, tool results cut to `preview` characters
/// (a failure keeps twice that).
#[must_use]
pub fn render_transcript(messages: &[Message], preview: usize) -> String {
    let mut out = String::new();
    for message in messages {
        let text = message.text.trim();
        match message.role {
            Role::User | Role::System if !text.is_empty() => out.push_str(&format!("User: {text}\n")),
            Role::Assistant => {
                if !text.is_empty() {
                    out.push_str(&format!("Assistant: {text}\n"));
                }
                for call in &message.tool_calls {
                    let args: String = call.args.to_string().chars().take(preview).collect();
                    out.push_str(&format!("Assistant called {}: {args}\n", call.name));
                }
            }
            Role::Tool => {
                let failed = text.starts_with("error:");
                let body: String = text.chars().take(if failed { preview * 2 } else { preview }).collect();
                out.push_str(&format!("Tool result ({}): {body}\n", if failed { "FAILED" } else { "ok" }));
            }
            _ => {}
        }
    }
    out
}

/// The summary framed as one message of established background.
#[must_use]
pub fn checkpoint(summary: &str) -> String {
    format!("{OPEN}\nThis is a checkpoint condensing an earlier part of this conversation. Treat it as established background and carry on from the messages after it without acknowledging it.\n\n{}\n{CLOSE}", summary.trim())
}

/// Replaces the first `cut` messages with `summary`. The checkpoint goes into the next message when that is the
/// user's (two user messages in a row are refused by some providers), and is a message of its own otherwise.
pub fn replace_head(history: &mut Vec<Message>, cut: usize, summary: &str) {
    let note = checkpoint(summary);
    let mut kept = history.split_off(cut);
    match kept.first_mut() {
        Some(next) if next.role == Role::User => next.text = format!("{note}\n\n{}", next.text),
        _ => kept.insert(0, Message::new(Role::User, note)),
    }
    *history = kept;
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;

    use super::*;

    #[test]
    fn a_summary_must_be_smaller_than_what_it_replaces() {
        assert!(shrinks("short", 100));
        assert!(!shrinks("   ", 100));
        assert!(!shrinks(&"x".repeat(400), 100));
    }

    #[test]
    fn the_head_becomes_one_checkpoint_and_the_tail_is_untouched() {
        let call = ToolCall { id: "c".into(), name: "read".into(), args: json!({"path": "a.rs"}) };
        let mut history = vec![
            Message::new(Role::User, "old"),
            Message::assistant("", vec![call]),
            Message::tool_result("c", "contents"),
            Message::new(Role::User, "new question"),
            Message::assistant("answer", vec![]),
        ];
        replace_head(&mut history, 3, "SUMMARY");
        assert_eq!(history.len(), 2, "the checkpoint joined the user message that followed");
        assert!(history[0].text.starts_with(OPEN) && history[0].text.contains("SUMMARY") && history[0].text.ends_with("new question"));
        let mut history = vec![Message::new(Role::User, "old"), Message::assistant("a", vec![]), Message::assistant("b", vec![])];
        replace_head(&mut history, 2, "SUMMARY");
        assert_eq!((history.len(), history[0].role), (2, Role::User), "an assistant message next gets a checkpoint of its own");
    }

    #[test]
    fn the_transcript_cuts_results_but_not_what_was_said() {
        let history = vec![Message::new(Role::User, "please fix it"), Message::tool_result("c", "y".repeat(900))];
        let text = render_transcript(&history, 100);
        assert!(text.contains("User: please fix it"));
        assert!(text.contains(&"y".repeat(100)) && !text.contains(&"y".repeat(101)));
    }
}
