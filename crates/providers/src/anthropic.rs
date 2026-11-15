//! Anthropic Messages API.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_core::{Message, Role};

use crate::sse::{DataParser, PartialCall, stream_events};
use crate::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};

const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 8192;

#[derive(Debug)]
pub struct Anthropic {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl Anthropic {
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self::with_base_url("https://api.anthropic.com", api_key)
    }

    #[must_use]
    pub fn with_base_url(base_url: &str, api_key: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for Anthropic {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&request_body(request))
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        stream_events(response, Box::new(AnthropicParser::default())).await
    }
}

fn request_body(req: &ChatRequest) -> Value {
    let mut body = json!({
        "model": req.model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "messages": messages_json(&req.messages),
    });
    if let Some(system) = &req.system {
        body["system"] = json!(system);
    }
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
            .collect();
    }
    body
}

/// Anthropic wants tool results as blocks of a user message, and all results
/// answering one assistant turn must share a single message.
fn messages_json(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::Tool => {
                let block = json!({"type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.text});
                match out.last_mut().filter(|l| is_tool_result_message(l)) {
                    Some(last) => last["content"]
                        .as_array_mut()
                        .into_iter()
                        .for_each(|c| c.push(block.clone())),
                    None => out.push(json!({"role": "user", "content": [block]})),
                }
            }
            Role::Assistant => {
                out.push(json!({"role": "assistant", "content": assistant_blocks(m)}));
            }
            Role::User | Role::System => {
                out.push(json!({"role": "user", "content": m.text}));
            }
        }
    }
    out
}

fn is_tool_result_message(v: &Value) -> bool {
    v["content"][0]["type"] == "tool_result"
}

fn assistant_blocks(m: &Message) -> Vec<Value> {
    let mut blocks = Vec::new();
    if !m.text.is_empty() {
        blocks.push(json!({"type": "text", "text": m.text}));
    }
    for c in &m.tool_calls {
        blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.args}));
    }
    blocks
}

#[derive(Default)]
struct AnthropicParser {
    /// Tool-use blocks being assembled, by content block index.
    calls: BTreeMap<u64, PartialCall>,
}

impl DataParser for AnthropicParser {
    fn feed(&mut self, data: &str) -> Result<Vec<ChatEvent>, ProviderError> {
        let v: Value =
            serde_json::from_str(data).map_err(|e| ProviderError::Parse(e.to_string()))?;
        let index = v["index"].as_u64().unwrap_or(0);
        Ok(match v["type"].as_str() {
            Some("content_block_start") if v["content_block"]["type"] == "tool_use" => {
                let block = &v["content_block"];
                let call = PartialCall {
                    id: block["id"].as_str().unwrap_or_default().to_owned(),
                    name: block["name"].as_str().unwrap_or_default().to_owned(),
                    args: String::new(),
                };
                self.calls.insert(index, call);
                vec![]
            }
            Some("content_block_delta") => match v["delta"]["type"].as_str() {
                Some("text_delta") => {
                    vec![ChatEvent::Text(
                        v["delta"]["text"].as_str().unwrap_or_default().to_owned(),
                    )]
                }
                Some("input_json_delta") => {
                    if let Some(call) = self.calls.get_mut(&index) {
                        call.args
                            .push_str(v["delta"]["partial_json"].as_str().unwrap_or_default());
                    }
                    vec![]
                }
                _ => vec![],
            },
            Some("content_block_stop") => self
                .calls
                .remove(&index)
                .map(PartialCall::into_event)
                .into_iter()
                .collect(),
            Some("error") => return Err(ProviderError::Parse(v["error"]["message"].to_string())),
            _ => vec![],
        })
    }

    fn finish(&mut self) -> Vec<ChatEvent> {
        vec![ChatEvent::Done]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vivianne_core::ToolCall;

    #[test]
    fn text_and_tool_use_blocks_stream_in_order() {
        let mut p = AnthropicParser::default();
        let mut out = Vec::new();
        for d in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"bash"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
        ] {
            out.extend(p.feed(d).unwrap());
        }
        out.extend(p.finish());
        let call = ToolCall {
            id: "t1".into(),
            name: "bash".into(),
            args: json!({"cmd": "ls"}),
        };
        assert_eq!(
            out,
            vec![
                ChatEvent::Text("ok".into()),
                ChatEvent::ToolCall(call),
                ChatEvent::Done
            ]
        );
    }

    #[test]
    fn consecutive_tool_results_share_one_user_message() {
        let msgs = vec![
            Message::new(Role::User, "go"),
            Message::assistant("", vec![]),
            Message::tool_result("a", "1"),
            Message::tool_result("b", "2"),
        ];
        let out = messages_json(&msgs);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2]["content"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn api_error_event_becomes_an_error() {
        let mut p = AnthropicParser::default();
        assert!(
            p.feed(r#"{"type":"error","error":{"message":"overloaded"}}"#)
                .is_err()
        );
    }
}
