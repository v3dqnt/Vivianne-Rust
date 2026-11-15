//! What the transcript shows, and how agent events change it. No GPUI here,
//! so all of this is unit tested.

use vivianne_runtime::AgentEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolItem {
    pub id: String,
    pub name: String,
    /// The arguments, shown on one line.
    pub summary: String,
    /// `None` while the tool is still running.
    pub output: Option<String>,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User(String),
    Assistant(String),
    Tool(ToolItem),
    Notice(String),
}

/// Fold one agent event into the transcript.
pub fn apply(items: &mut Vec<Item>, event: &AgentEvent) {
    match event {
        AgentEvent::Text(piece) => match items.last_mut() {
            Some(Item::Assistant(text)) => text.push_str(piece),
            _ => items.push(Item::Assistant(piece.clone())),
        },
        AgentEvent::ToolStart { id, name, args } => items.push(Item::Tool(ToolItem {
            id: id.clone(),
            name: name.clone(),
            summary: args.to_string(),
            output: None,
            is_error: false,
        })),
        AgentEvent::ToolEnd {
            id,
            output,
            is_error,
            ..
        } => {
            let running = items.iter_mut().rev().find_map(|item| match item {
                Item::Tool(tool) if tool.id == *id => Some(tool),
                _ => None,
            });
            if let Some(tool) = running {
                tool.output = Some(output.clone());
                tool.is_error = *is_error;
            }
        }
        AgentEvent::StepLimit => {
            items.push(Item::Notice("Stopped: step limit reached.".to_owned()));
        }
        AgentEvent::Done => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_start(id: &str) -> AgentEvent {
        AgentEvent::ToolStart {
            id: id.into(),
            name: "shell".into(),
            args: json!({"command": "ls"}),
        }
    }

    fn tool_end(id: &str, output: &str, is_error: bool) -> AgentEvent {
        AgentEvent::ToolEnd {
            id: id.into(),
            name: "shell".into(),
            output: output.into(),
            is_error,
        }
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
    fn tool_end_fills_in_the_matching_tool() {
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
        assert_eq!(second.output, None);
    }

    #[test]
    fn step_limit_adds_a_notice() {
        let mut items = Vec::new();
        apply(&mut items, &AgentEvent::StepLimit);
        assert!(matches!(items.as_slice(), [Item::Notice(_)]));
    }
}
