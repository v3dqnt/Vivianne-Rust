//! Server-sent-events plumbing shared by all adapters.

use std::collections::VecDeque;

use bytes::Bytes;
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use serde_json::Value;
use vivianne_core::ToolCall;

use crate::{ChatEvent, EventStream, ProviderError};

/// Splits a byte stream into the payloads of its `data:` lines.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                out.push(data.trim_start().to_owned());
            }
        }
        out
    }
}

/// Turns provider-specific JSON events into [`ChatEvent`]s.
pub trait DataParser: Send {
    fn feed(&mut self, data: &str) -> Result<Vec<ChatEvent>, ProviderError>;
    /// Called once when the body ends; must end the stream with `Done`.
    fn finish(&mut self) -> Vec<ChatEvent>;
}

/// A tool call being assembled from streamed fragments.
#[derive(Default)]
pub struct PartialCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

impl PartialCall {
    pub fn into_event(self) -> ChatEvent {
        // Models sometimes emit broken JSON; keep the raw text so the tool call fails visibly.
        let args = if self.args.trim().is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(&self.args).unwrap_or(Value::String(self.args))
        };
        ChatEvent::ToolCall(ToolCall {
            id: self.id,
            name: self.name,
            args,
        })
    }
}

struct State {
    body: BoxStream<'static, reqwest::Result<Bytes>>,
    decoder: SseDecoder,
    parser: Box<dyn DataParser>,
    queue: VecDeque<ChatEvent>,
    finished: bool,
}

/// Check the status, then stream the body through `parser`.
pub async fn stream_events(
    response: reqwest::Response,
    parser: Box<dyn DataParser>,
) -> Result<EventStream, ProviderError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Api {
            status: status.as_u16(),
            body,
        });
    }
    let state = State {
        body: response.bytes_stream().boxed(),
        decoder: SseDecoder::default(),
        parser,
        queue: VecDeque::new(),
        finished: false,
    };
    Ok(Box::pin(stream::unfold(state, next_event)))
}

async fn next_event(mut s: State) -> Option<(Result<ChatEvent, ProviderError>, State)> {
    loop {
        if let Some(event) = s.queue.pop_front() {
            return Some((Ok(event), s));
        }
        if s.finished {
            return None;
        }
        match s.body.next().await {
            Some(Ok(bytes)) => {
                for data in s.decoder.push(&bytes) {
                    match s.parser.feed(&data) {
                        Ok(events) => s.queue.extend(events),
                        Err(e) => {
                            s.finished = true;
                            return Some((Err(e), s));
                        }
                    }
                }
            }
            Some(Err(e)) => {
                s.finished = true;
                return Some((Err(ProviderError::Http(e.to_string())), s));
            }
            None => {
                s.finished = true;
                let tail = s.parser.finish();
                s.queue.extend(tail);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_returns_data_lines_and_ignores_others() {
        let mut d = SseDecoder::default();
        let out = d.push(b"event: x\ndata: {\"a\":1}\n\ndata: [DONE]\n");
        assert_eq!(out, vec![r#"{"a":1}"#, "[DONE]"]);
    }

    #[test]
    fn decoder_waits_for_a_full_line_even_mid_character() {
        let mut d = SseDecoder::default();
        let bytes = "data: héllo\n".as_bytes();
        assert!(d.push(&bytes[..9]).is_empty()); // cuts inside the two-byte é
        assert_eq!(d.push(&bytes[9..]), vec!["héllo"]);
    }

    #[test]
    fn broken_tool_args_are_kept_as_raw_text() {
        let call = PartialCall {
            id: "1".into(),
            name: "x".into(),
            args: "{oops".into(),
        };
        let ChatEvent::ToolCall(c) = call.into_event() else {
            unreachable!()
        };
        assert_eq!(c.args, Value::String("{oops".into()));
    }
}
