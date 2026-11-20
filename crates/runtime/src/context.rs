//! Where a conversation's context window is going, in the parts a user can act on: trim the history, turn off
//! tools, or compact.
//!
//! Every figure uses one estimate (a token is about four characters), so the parts add up to the whole instead of
//! being measured two ways that disagree. The old app counted some parts in characters and others in bytes, so
//! text that is not plain English (accents, Chinese, emoji) made the system prompt look bigger than the messages.

use serde_json::Value;
use vivianne_core::{Message, Role};

/// Context tokens for `text`: four characters to a token, rounded up.
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from(text.chars().count().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Context tokens for one message: its text, and for an assistant message the name and arguments of its calls.
#[must_use]
pub fn message_tokens(message: &Message) -> u32 {
    message.tool_calls.iter().fold(estimate_tokens(&message.text), |sum, call| {
        sum.saturating_add(estimate_tokens(&call.name)).saturating_add(estimate_tokens(&call.args.to_string()))
    })
}

/// The window's fill, by what is in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// The standing instructions, including the current mode's.
    pub system: u32,
    /// The definitions of the tools the model may use. They ride along with every request.
    pub tools: u32,
    /// What the user said.
    pub user: u32,
    /// What the model said, including the arguments of its tool calls.
    pub assistant: u32,
    /// What the tools answered.
    pub tool_results: u32,
}

impl Breakdown {
    #[must_use]
    pub fn total(&self) -> u32 {
        self.system.saturating_add(self.tools).saturating_add(self.user).saturating_add(self.assistant).saturating_add(self.tool_results)
    }

    /// The parts that have anything in them, in the order a user reads them: (label, tokens).
    #[must_use]
    pub fn rows(&self) -> Vec<(&'static str, u32)> {
        [("System prompt", self.system), ("Tool definitions", self.tools), ("You", self.user), ("Assistant", self.assistant), ("Tool results", self.tool_results)]
            .into_iter()
            .filter(|(_, tokens)| *tokens > 0)
            .collect()
    }
}

/// The breakdown of one request: the system prompt, the tool definitions (as JSON, the way they are sent) and the
/// conversation.
#[must_use]
pub fn breakdown(system: &str, tools: &[Value], history: &[Message]) -> Breakdown {
    let mut parts = Breakdown {
        system: estimate_tokens(system),
        tools: tools.iter().map(|t| estimate_tokens(&t.to_string())).fold(0, u32::saturating_add),
        ..Breakdown::default()
    };
    for message in history {
        let slot = match message.role {
            Role::Assistant => &mut parts.assistant,
            Role::Tool => &mut parts.tool_results,
            // The system role is part of what the user (or the app) put in.
            Role::User | Role::System => &mut parts.user,
        };
        *slot = slot.saturating_add(message_tokens(message));
    }
    parts
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;

    use super::*;

    #[test]
    fn a_token_is_four_characters_counted_as_characters_not_bytes() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // Four characters, twelve bytes: still one token.
        assert_eq!(estimate_tokens("日本語です"), 2);
        assert_eq!(estimate_tokens("éééé"), 1);
    }

    #[test]
    fn each_part_is_counted_in_its_own_slot_and_the_parts_add_up() {
        let call = ToolCall { id: "c".into(), name: "read".into(), args: json!({"path": "a.rs"}) };
        let history = vec![
            Message::new(Role::User, "abcdefgh"),
            Message::assistant("abcd", vec![call.clone()]),
            Message::tool_result("c", "abcdefghijkl"),
        ];
        let tools = [json!({"name": "read"})];
        let parts = breakdown("abcdefgh", &tools, &history);
        assert_eq!((parts.system, parts.user, parts.tool_results), (2, 2, 3));
        // The call's name and arguments count as what the assistant said.
        assert_eq!(parts.assistant, 1 + estimate_tokens("read") + estimate_tokens(&call.args.to_string()));
        assert_eq!(parts.tools, estimate_tokens(&tools[0].to_string()));
        assert_eq!(parts.total(), parts.system + parts.tools + parts.user + parts.assistant + parts.tool_results);
    }

    #[test]
    fn rows_leave_out_what_is_empty() {
        let parts = Breakdown { user: 5, tool_results: 2, ..Breakdown::default() };
        assert_eq!(parts.rows(), vec![("You", 5), ("Tool results", 2)]);
    }
}
