//! OpenAI-compatible chat completions: OpenAI, Google's compatible endpoint,
//! Ollama, llama.cpp and similar.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_core::{Message, Role};

use crate::sse::{DataParser, PartialCall, stream_events};
use crate::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};

#[derive(Debug)]
pub struct OpenAiCompatible {
    id: String,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAiCompatible {
    /// `base_url` like `https://api.openai.com/v1`, without a trailing slash.
    #[must_use]
    pub fn new(id: &str, base_url: &str, api_key: Option<String>) -> Self {
        Self {
            id: id.to_owned(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatible {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
        let mut call = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&request_body(request));
        if let Some(key) = &self.api_key {
            call = call.bearer_auth(key);
        }
        let response = call
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        stream_events(response, Box::new(OpenAiParser::default())).await
    }
}

fn request_body(req: &ChatRequest) -> Value {
    let system = req
        .system
        .iter()
        .map(|s| json!({"role": "system", "content": s}));
    let messages: Vec<Value> = system
        .chain(req.messages.iter().map(message_json))
        .collect();
    let mut body = json!({"model": req.model, "messages": messages, "stream": true});
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters}})
            })
            .collect();
    }
    body
}

fn message_json(m: &Message) -> Value {
    match m.role {
        Role::Tool => json!({
            "role": "tool",
            "tool_call_id": m.tool_call_id,
            "content": m.text,
        }),
        Role::Assistant if !m.tool_calls.is_empty() => json!({
            "role": "assistant",
            "content": m.text,
            "tool_calls": m.tool_calls.iter().map(|c| json!({
                "id": c.id,
                "type": "function",
                "function": {"name": c.name, "arguments": c.args.to_string()},
            })).collect::<Vec<_>>(),
        }),
        Role::Assistant => json!({"role": "assistant", "content": m.text}),
        Role::User => json!({"role": "user", "content": m.text}),
        Role::System => json!({"role": "system", "content": m.text}),
    }
}

#[derive(Default)]
struct OpenAiParser {
    calls: BTreeMap<u64, PartialCall>,
}

impl OpenAiParser {
    fn flush(&mut self) -> Vec<ChatEvent> {
        std::mem::take(&mut self.calls)
            .into_values()
            .map(PartialCall::into_event)
            .collect()
    }
}

impl DataParser for OpenAiParser {
    fn feed(&mut self, data: &str) -> Result<Vec<ChatEvent>, ProviderError> {
        if data == "[DONE]" {
            return Ok(self.flush());
        }
        let v: Value =
            serde_json::from_str(data).map_err(|e| ProviderError::Parse(e.to_string()))?;
        let choice = &v["choices"][0];
        let delta = &choice["delta"];
        let mut out = Vec::new();
        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            out.push(ChatEvent::Text(text.to_owned()));
        }
        for fragment in delta["tool_calls"].as_array().into_iter().flatten() {
            let call = self
                .calls
                .entry(fragment["index"].as_u64().unwrap_or(0))
                .or_default();
            if let Some(id) = fragment["id"].as_str() {
                id.clone_into(&mut call.id);
            }
            if let Some(name) = fragment["function"]["name"].as_str() {
                call.name.push_str(name);
            }
            if let Some(args) = fragment["function"]["arguments"].as_str() {
                call.args.push_str(args);
            }
        }
        if choice["finish_reason"].is_string() {
            out.extend(self.flush());
        }
        Ok(out)
    }

    fn finish(&mut self) -> Vec<ChatEvent> {
        let mut out = self.flush();
        out.push(ChatEvent::Done);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vivianne_core::ToolCall;

    fn run(chunks: &[&str]) -> Vec<ChatEvent> {
        let mut p = OpenAiParser::default();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.feed(c).unwrap());
        }
        out.extend(p.finish());
        out
    }

    #[test]
    fn text_deltas_stream_through() {
        let events = run(&[
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            "[DONE]",
        ]);
        assert_eq!(
            events,
            vec![
                ChatEvent::Text("Hel".into()),
                ChatEvent::Text("lo".into()),
                ChatEvent::Done
            ]
        );
    }

    #[test]
    fn tool_call_fragments_are_joined_into_one_call() {
        let events = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":"{\"cmd\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        let call = ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            args: json!({"cmd": "ls"}),
        };
        assert_eq!(events, vec![ChatEvent::ToolCall(call), ChatEvent::Done]);
    }

    #[test]
    fn body_has_system_first_and_tool_results_linked() {
        let req = ChatRequest {
            model: "m".into(),
            system: Some("be brief".into()),
            messages: vec![
                Message::new(Role::User, "hi"),
                Message::tool_result("c1", "done"),
            ],
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["tool_call_id"], "c1");
        assert!(body.get("tools").is_none());
    }
}
