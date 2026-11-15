//! An offline provider that replays a canned reply, so the window can be run
//! and checked without an API key. Start the app with `VIV_DEMO=1`.

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use serde_json::json;
use vivianne_core::{Role, ToolCall};
use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};

const OPENING: &str = "I'll look around the project first.";
const CLOSING: &str = "That is the lay of the land: a Rust workspace with a native shell, an agent runtime and the tools.";
const PIECE_CHARS: usize = 6;
const PIECE_DELAY: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct Demo;

#[async_trait]
impl Provider for Demo {
    fn id(&self) -> &'static str {
        "demo"
    }

    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
        let after_tool = request
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Tool);
        let mut events = pieces(if after_tool { CLOSING } else { OPENING });
        if !after_tool {
            let tour = [
                ("read", json!({"path": "Cargo.toml", "limit": 30})),
                ("ls", json!({"path": "crates", "depth": 2})),
                ("grep", json!({"pattern": "fn main", "path": "crates"})),
                ("glob", json!({"pattern": "*.toml"})),
                (
                    "code_search",
                    json!({"query": "agent loop that runs tools"}),
                ),
                ("repo_overview", json!({"path": ".", "depth": 2})),
                ("diagnostics", json!({"path": "Cargo.toml"})),
                ("lsp", json!({"operation": "status"})),
            ];
            for (index, (name, args)) in tour.into_iter().enumerate() {
                events.push(ChatEvent::ToolCall(ToolCall {
                    id: format!("demo-{index}"),
                    name: name.to_owned(),
                    args,
                }));
            }
        }
        events.push(ChatEvent::Done);
        let paced = stream::iter(events).then(|event| async move {
            tokio::time::sleep(PIECE_DELAY).await;
            Ok(event)
        });
        Ok(Box::pin(paced))
    }
}

fn pieces(text: &str) -> Vec<ChatEvent> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(PIECE_CHARS)
        .map(|c| ChatEvent::Text(c.iter().collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_rebuild_the_original_text() {
        let joined: String = pieces(OPENING)
            .into_iter()
            .map(|e| match e {
                ChatEvent::Text(t) => t,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(joined, OPENING);
    }
}
