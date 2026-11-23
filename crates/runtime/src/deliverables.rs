//! What a conversation handed to the user on purpose: the files the model declared with the `present` tool. The declaration
//! is durable without a store of its own: the call and its result are in the saved history, and this reads them back.
//! A call that failed declared nothing.

use serde::{Deserialize, Serialize};
use vivianne_core::{Message, Role};

const RESULT_PREFIX: &str = "Presented ";

/// A file the model declared as final: the path as the tool resolved it, and its description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedFile {
    /// Absolute: later changes of the working folder do not redirect it.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The files of one successful `present` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presented {
    /// The turn (the person's message, counting from 1) the call was made in.
    pub turn: usize,
    pub call_id: String,
    pub files: Vec<PresentedFile>,
}

/// Every successful declaration in `history`, in order.
#[must_use]
pub fn presented(history: &[Message]) -> Vec<Presented> {
    let mut turn = 0;
    let mut out = Vec::new();
    for (index, message) in history.iter().enumerate() {
        if message.is_typed() {
            turn += 1;
        }
        if message.role != Role::Assistant {
            continue;
        }
        for call in message
            .tool_calls
            .iter()
            .filter(|c| c.name == vivianne_tools::present::NAME)
        {
            let Some(result) = history[index + 1..].iter().find(|m| {
                m.role == Role::Tool && m.tool_call_id.as_deref() == Some(call.id.as_str())
            }) else {
                continue;
            };
            if result.text.starts_with("error:") {
                continue;
            }
            let described = call
                .args
                .get("files")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            let files: Vec<PresentedFile> = result
                .text
                .lines()
                .filter_map(|line| line.strip_prefix(RESULT_PREFIX))
                .enumerate()
                .map(|(i, path)| PresentedFile {
                    path: path.to_owned(),
                    description: described
                        .get(i)
                        .and_then(|f| f.get("description"))
                        .and_then(|d| d.as_str())
                        .map(str::to_owned),
                })
                .collect();
            if !files.is_empty() {
                out.push(Presented {
                    turn: turn.max(1),
                    call_id: call.id.clone(),
                    files,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vivianne_core::ToolCall;

    #[allow(clippy::needless_pass_by_value)]
    fn call(id: &str, files: serde_json::Value) -> Message {
        Message::assistant(
            String::new(),
            vec![ToolCall {
                id: id.into(),
                name: "present".into(),
                args: json!({ "files": files }),
            }],
        )
    }

    #[test]
    fn a_successful_call_is_read_back_with_its_turn_paths_and_descriptions() {
        let history = vec![
            Message::new(Role::User, "make a deck"),
            call(
                "c1",
                json!([{ "path": "deck.pptx", "description": "the deck" }, { "path": "notes.md" }]),
            ),
            Message::tool_result(
                "c1",
                "Presented /w/deck.pptx\nPresented /w/notes.md".to_owned(),
            ),
            Message::new(Role::User, "and a report"),
            call("c2", json!([{ "path": "r.pdf" }])),
            Message::tool_result("c2", "Presented /w/r.pdf".to_owned()),
        ];
        let got = presented(&history);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].turn, got[0].call_id.as_str()), (1, "c1"));
        assert_eq!(
            got[0].files[0],
            PresentedFile {
                path: "/w/deck.pptx".into(),
                description: Some("the deck".into())
            }
        );
        assert_eq!(got[0].files[1].description, None);
        assert_eq!(got[1].turn, 2);
    }

    #[test]
    fn failed_unanswered_and_other_calls_declare_nothing() {
        let history = vec![
            Message::new(Role::User, "go"),
            call("bad", json!([{ "path": "x" }])),
            Message::tool_result("bad", "error: Cannot present x: file not found".to_owned()),
            call("open", json!([{ "path": "y" }])),
            Message::assistant(
                String::new(),
                vec![ToolCall {
                    id: "o".into(),
                    name: "read".into(),
                    args: json!({ "path": "z" }),
                }],
            ),
            Message::tool_result("o", "Presented /not/from/present".to_owned()),
        ];
        assert!(presented(&history).is_empty());
    }

    #[test]
    fn messages_the_app_added_do_not_count_as_turns() {
        let history = vec![
            Message::new(Role::User, "go"),
            Message::context(
                "<system-reminder>x</system-reminder>",
                vivianne_core::context::ContextSource::TimeContext,
            ),
            call("c", json!([{ "path": "a" }])),
            Message::tool_result("c", "Presented /w/a".to_owned()),
        ];
        assert_eq!(presented(&history)[0].turn, 1);
    }
}
