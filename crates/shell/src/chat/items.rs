//! What the transcript shows, and how agent events change it. No GPUI here,
//! so all of this is unit tested.

use serde_json::Value;
use vivianne_runtime::AgentEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolItem {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// `None` while the tool is still running.
    pub output: Option<String>,
    pub is_error: bool,
    /// What the tool reported beyond its text (the original tools' `details`).
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User(String),
    Assistant(String),
    Tool(ToolItem),
    Notice(String),
    /// A coding agent asks to do something. `answer` is `None` until the user decides.
    Approval { id: String, title: String, answer: Option<bool> },
}

/// The transcript of a saved conversation: what was said, and each tool call with what it answered. The
/// condensed-history checkpoint is not something anyone said, so it is left out.
#[must_use]
pub fn from_history(history: &[vivianne_core::Message]) -> Vec<Item> {
    use vivianne_core::Role;
    let mut items = Vec::new();
    for message in history {
        match message.role {
            Role::User if message.text.trim_start().starts_with("<compacted_history>") => {}
            Role::User => items.push(Item::User(message.text.clone())),
            Role::Assistant => {
                if !message.text.trim().is_empty() {
                    items.push(Item::Assistant(message.text.clone()));
                }
                for call in &message.tool_calls {
                    let answer = history.iter().find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(&call.id));
                    items.push(Item::Tool(ToolItem {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        args: call.args.clone(),
                        is_error: answer.is_some_and(|a| a.text.starts_with("error: ")),
                        output: answer.map(|a| a.text.clone()),
                        details: None,
                    }));
                }
            }
            Role::Tool | Role::System => {}
        }
    }
    items
}

/// What one event did to the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Nothing,
    /// A new item at the end.
    Appended,
    /// An item that was already there grew or finished.
    Updated(usize),
}

/// Records the user's answer to an approval; the index of the item, if it was waiting.
pub fn answer_approval(items: &mut [Item], id: &str, allow: bool) -> Option<usize> {
    let found = items.iter_mut().enumerate().find_map(|(index, item)| match item {
        Item::Approval { id: waiting, answer, .. } if waiting == id && answer.is_none() => Some((index, answer)),
        _ => None,
    })?;
    *found.1 = Some(allow);
    Some(found.0)
}

/// Fold one agent event into the transcript.
pub fn apply(items: &mut Vec<Item>, event: &AgentEvent) -> Change {
    match event {
        AgentEvent::Text(piece) => match items.last_mut() {
            Some(Item::Assistant(text)) => {
                text.push_str(piece);
                Change::Updated(items.len() - 1)
            }
            _ => {
                items.push(Item::Assistant(piece.clone()));
                Change::Appended
            }
        },
        AgentEvent::ToolStart { id, name, args } => {
            items.push(Item::Tool(ToolItem {
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
                output: None,
                is_error: false,
                details: None,
            }));
            Change::Appended
        }
        AgentEvent::ToolEnd {
            id,
            output,
            is_error,
            details,
            args,
            ..
        } => {
            let running = items.iter_mut().enumerate().rev().find_map(|(index, item)| match item {
                Item::Tool(tool) if tool.id == *id => Some((index, tool)),
                _ => None,
            });
            let Some((index, tool)) = running else {
                return Change::Nothing;
            };
            tool.output = Some(output.clone());
            tool.is_error = *is_error;
            tool.details.clone_from(details);
            if let Some(args) = args {
                tool.args.clone_from(args);
            }
            Change::Updated(index)
        }
        AgentEvent::Approval { id, title } => {
            items.push(Item::Approval { id: id.clone(), title: title.clone(), answer: None });
            Change::Appended
        }
        AgentEvent::StepLimit => {
            items.push(Item::Notice("Stopped: step limit reached.".to_owned()));
            Change::Appended
        }
        AgentEvent::Fallback { to, reason, .. } => {
            items.push(Item::Notice(format!("Switched to {to}: {reason}")));
            Change::Appended
        }
        AgentEvent::Compacted { before, after } => {
            items.push(Item::Notice(format!("Condensed the earlier conversation to make room ({before} to {after} tokens).")));
            Change::Appended
        }
        AgentEvent::Retrying { .. } | AgentEvent::Usage { .. } | AgentEvent::Stopped | AgentEvent::Done => Change::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_start(id: &str) -> AgentEvent {
        AgentEvent::ToolStart {
            id: id.into(),
            name: "grep".into(),
            args: json!({"pattern": "x"}),
        }
    }

    fn tool_end(id: &str, output: &str, is_error: bool) -> AgentEvent {
        AgentEvent::ToolEnd {
            id: id.into(),
            name: "grep".into(),
            output: output.into(),
            is_error,
            details: Some(json!({"matches": 1})),
            args: None,
        }
    }

    #[test]
    fn a_saved_conversation_comes_back_as_messages_and_tool_rows_without_the_checkpoint() {
        use vivianne_core::{Message, Role, ToolCall};
        let call = ToolCall { id: "c".into(), name: "read".into(), args: json!({"path": "a.rs"}) };
        let history = vec![
            Message::new(Role::User, "<compacted_history>\nsummary"),
            Message::new(Role::User, "look at a.rs"),
            Message::assistant("ok", vec![call]),
            Message::tool_result("c", "error: no such file"),
            Message::assistant("it is missing", vec![]),
        ];
        let items = from_history(&history);
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[0], Item::User(t) if t == "look at a.rs"));
        assert!(matches!(&items[2], Item::Tool(t) if t.is_error && t.output.as_deref() == Some("error: no such file")));
        assert!(matches!(&items[3], Item::Assistant(t) if t == "it is missing"));
    }

    #[test]
    fn text_pieces_grow_one_assistant_item() {
        let mut items = vec![Item::User("hi".into())];
        apply(&mut items, &AgentEvent::Text("Hel".into()));
        apply(&mut items, &AgentEvent::Text("lo".into()));
        assert_eq!(
            items,
            vec![Item::User("hi".into()), Item::Assistant("Hello".into())]
        );
    }

    #[test]
    fn text_after_a_tool_starts_a_new_assistant_item() {
        let mut items = Vec::new();
        apply(&mut items, &AgentEvent::Text("a".into()));
        apply(&mut items, &tool_start("1"));
        apply(&mut items, &AgentEvent::Text("b".into()));
        assert_eq!(items.len(), 3);
        assert_eq!(items[2], Item::Assistant("b".into()));
    }

    #[test]
    fn tool_end_fills_in_the_matching_tool_with_its_details() {
        let mut items = Vec::new();
        apply(&mut items, &tool_start("1"));
        apply(&mut items, &tool_start("2"));
        apply(&mut items, &tool_end("1", "boom", true));
        let Item::Tool(first) = &items[0] else {
            panic!("not a tool")
        };
        let Item::Tool(second) = &items[1] else {
            panic!("not a tool")
        };
        assert_eq!(
            (first.output.as_deref(), first.is_error),
            (Some("boom"), true)
        );
        assert_eq!(first.details, Some(json!({"matches": 1})));
        assert_eq!(second.output, None);
        assert_eq!(first.args["pattern"], "x");
    }

    #[test]
    fn each_event_says_what_it_changed() {
        let mut items = Vec::new();
        assert_eq!(apply(&mut items, &AgentEvent::Text("a".into())), Change::Appended);
        assert_eq!(apply(&mut items, &AgentEvent::Text("b".into())), Change::Updated(0));
        assert_eq!(apply(&mut items, &tool_start("1")), Change::Appended);
        assert_eq!(apply(&mut items, &tool_end("1", "ok", false)), Change::Updated(1));
        assert_eq!(apply(&mut items, &tool_end("gone", "ok", false)), Change::Nothing);
        assert_eq!(apply(&mut items, &AgentEvent::Done), Change::Nothing);
    }

    #[test]
    fn step_limit_adds_a_notice() {
        let mut items = Vec::new();
        apply(&mut items, &AgentEvent::StepLimit);
        assert!(matches!(items.as_slice(), [Item::Notice(_)]));
    }

    #[test]
    fn a_tool_end_with_arguments_replaces_the_ones_from_its_start() {
        let mut items = Vec::new();
        apply(&mut items, &tool_start("t"));
        let end = AgentEvent::ToolEnd {
            id: "t".into(),
            name: "edit".into(),
            output: "ok".into(),
            is_error: false,
            details: None,
            args: Some(json!({"old_string": "a", "new_string": "b"})),
        };
        apply(&mut items, &end);
        let Item::Tool(tool) = &items[0] else { panic!("not a tool") };
        assert_eq!(tool.args["new_string"], "b");
    }

    #[test]
    fn an_approval_waits_until_answered_and_is_answered_once() {
        let mut items = Vec::new();
        let ask = AgentEvent::Approval { id: "a1".into(), title: "Run tests".into() };
        assert_eq!(apply(&mut items, &ask), Change::Appended);
        assert_eq!(answer_approval(&mut items, "a1", true), Some(0));
        assert_eq!(answer_approval(&mut items, "a1", false), None);
        assert_eq!(answer_approval(&mut items, "other", true), None);
        assert!(matches!(&items[0], Item::Approval { answer: Some(true), .. }));
    }
}
