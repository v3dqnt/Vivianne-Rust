//! An offline provider that replays a canned reply, so the window can be run
//! and checked without an API key. Start the app with `VIV_DEMO=1`.

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use serde_json::json;
use vivianne_core::{Role, ToolCall};
use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};

const OPENING: &str = "I'll check the project folder first.\n\n```sh\nls\n```\n\nRunning it now.";
const CLOSING: &str = "Done. The folder lists the workspace crates, docs and config files. \
Nothing needed changing.";
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
            events.push(ChatEvent::ToolCall(ToolCall {
                id: "demo-1".to_owned(),
                name: "shell".to_owned(),
                args: json!({"command": "ls"}),
            }));
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
