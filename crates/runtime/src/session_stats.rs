//! Whole-conversation figures and a turn-by-turn outline, read from the saved history. Port of the original `session-stats`
//! and `session-turn-outline` projections: both are folds of the complete log, so they do not change when a client has only
//! part of it loaded or when old messages were condensed into a checkpoint.
//!
//! Timing comes from the moments the agent loop stamps on its own messages (`Message::at`): a model call is the time from
//! the last thing sent to the reply, a tool batch the time from the reply to its results. The first-token and decode
//! times are the ones the agent loop measured on the stream itself (`Message::timing`); replies saved before they were
//! recorded add nothing to them.

use serde::{Deserialize, Serialize};
use vivianne_core::{Message, Role};

/// One prompt line.
pub const PROMPT_PREVIEW: usize = 50;
/// Up to three lines of the final answer.
pub const RESPONSE_PREVIEW: usize = 120;

/// Figures for a whole conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Turns that have at least one model reply.
    pub turns: usize,
    /// Model replies (each is one step: one model call).
    pub steps: usize,
    /// Tool calls the model made.
    pub tool_calls: usize,
    /// Tool results that said they failed.
    pub failed_tool_calls: usize,
    /// Messages the app added for the model (instruction files, the clock, ...).
    pub context_messages: usize,
    /// Model wall time over the steps that carry times.
    pub llm_ms: u64,
    /// Tool wall time over the batches that carry times.
    pub tool_ms: u64,
    /// Waiting for the first thing back, summed over the replies that were timed.
    pub first_token_ms: u64,
    /// Streaming the rest of those replies.
    pub decode_ms: u64,
    /// How many replies carry a measured timing (the base the two figures above cover).
    pub timed_steps: usize,
}

/// The moment a message was stamped, when it was.
fn at(message: &Message) -> Option<u64> {
    message.at
}

/// The figures as they are being folded: what [`Stats`] is, plus the moments the fold is holding on to between messages.
/// Kept apart from [`Stats`] so a saved copy can take up again where it stopped (see [`crate::projection`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsFold {
    out: Stats,
    turns_with_a_reply: std::collections::BTreeSet<usize>,
    turn: usize,
    /// When the last thing the model was given happened (the message it is answering).
    sent_at: Option<u64>,
    /// The tool batch in flight: when the reply that asked for it came, and when its newest result did.
    batch: Option<(u64, u64)>,
    asked_at: Option<u64>,
}

impl StatsFold {
    /// Takes in the next message of the history.
    pub fn push(&mut self, message: &Message) {
        if message.role != Role::Tool
            && let Some((from, last)) = self.batch.take()
        {
            self.out.tool_ms += last.saturating_sub(from);
        }
        match message.role {
            Role::User if message.context.is_some() => {
                self.out.context_messages += 1;
                self.sent_at = at(message).or(self.sent_at);
            }
            Role::User => {
                self.turn += 1;
                self.sent_at = at(message).or(self.sent_at);
            }
            Role::Assistant => {
                self.out.steps += 1;
                self.turns_with_a_reply.insert(self.turn);
                self.out.tool_calls += message.tool_calls.len();
                if let Some(timing) = message.timing {
                    self.out.first_token_ms += timing.first_token_ms;
                    self.out.decode_ms += timing.decode_ms;
                    self.out.timed_steps += 1;
                }
                if let (Some(sent), Some(replied)) = (self.sent_at, at(message)) {
                    self.out.llm_ms += replied.saturating_sub(sent);
                }
                self.asked_at = at(message);
                self.sent_at = at(message).or(self.sent_at);
            }
            Role::Tool => {
                if message.text.starts_with("error:") {
                    self.out.failed_tool_calls += 1;
                }
                if let (Some(from), Some(done)) = (self.asked_at, at(message)) {
                    self.batch = Some((self.batch.map_or(from, |(f, _)| f), done));
                    self.sent_at = Some(done);
                }
            }
            Role::System => {}
        }
    }

    /// The figures so far; a tool batch still open counts as ended where its newest result was.
    #[must_use]
    pub fn finish(&self) -> Stats {
        let mut out = self.out;
        if let Some((from, last)) = self.batch {
            out.tool_ms += last.saturating_sub(from);
        }
        out.turns = self.turns_with_a_reply.len();
        out
    }
}

/// Folds the whole history into [`Stats`].
#[must_use]
pub fn stats(history: &[Message]) -> Stats {
    let mut fold = StatsFold::default();
    for message in history {
        fold.push(message);
    }
    fold.finish()
}

/// One turn as an outline shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineEntry {
    /// From 1.
    pub turn: usize,
    /// The position of what the person typed; paging back to it loads the whole turn.
    pub seq: usize,
    /// What the person asked, on one line.
    pub prompt: String,
    /// The turn's last answer with text; empty until there is one.
    pub response: String,
}

/// One line of text: whitespace collapsed, clipped with an ellipsis when it does not fit `limit` characters.
#[must_use]
pub fn preview(text: &str, limit: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > limit.saturating_sub(1) {
        let kept: String = normalized.chars().take(limit.saturating_sub(1)).collect();
        return format!("{}…", kept.trim_end());
    }
    normalized
}

/// What the person typed, without the condensed-history checkpoint that may be attached in front of it.
fn typed_text(message: &Message) -> &str {
    const CLOSE: &str = "</compacted_history>";
    let text = message.text.trim_start();
    if text.starts_with("<compacted_history>") {
        return text
            .find(CLOSE)
            .map_or("", |end| &text[end + CLOSE.len()..]);
    }
    &message.text
}

/// The outline as it is being folded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineFold {
    entries: Vec<OutlineEntry>,
    /// The position of the next message.
    seq: usize,
}

impl OutlineFold {
    /// Takes in the next message of the history.
    pub fn push(&mut self, message: &Message) {
        if message.is_typed() {
            self.entries.push(OutlineEntry {
                turn: self.entries.len() + 1,
                seq: self.seq,
                prompt: preview(typed_text(message), PROMPT_PREVIEW),
                response: String::new(),
            });
        } else if message.role == Role::Assistant
            && !message.text.trim().is_empty()
            && let Some(entry) = self.entries.last_mut()
        {
            entry.response = preview(&message.text, RESPONSE_PREVIEW);
        }
        self.seq += 1;
    }

    #[must_use]
    pub fn finish(&self) -> Vec<OutlineEntry> {
        self.entries.clone()
    }
}

/// Every turn, in order, with its bounded previews. Injected context never appears in them.
#[must_use]
pub fn outline(history: &[Message]) -> Vec<OutlineEntry> {
    let mut fold = OutlineFold::default();
    for message in history {
        fold.push(message);
    }
    fold.finish()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;
    use vivianne_core::context::ContextSource;

    use super::*;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "read".into(),
            args: json!({ "path": "a" }),
        }
    }

    fn timed() -> Vec<Message> {
        vec![
            Message::new(Role::User, "first question").at(1_000),
            Message::context(
                "<system-reminder>x</system-reminder>",
                ContextSource::TimeContext,
            )
            .at(1_100),
            Message::assistant(String::new(), vec![call("a"), call("b")]).at(1_600),
            Message::tool_result("a", "text".to_owned()).at(2_000),
            Message::tool_result("b", "error: nope".to_owned()).at(2_000),
            Message::assistant("the answer".to_owned(), Vec::new()).at(2_500),
            Message::new(Role::User, "second").at(3_000),
        ]
    }

    #[test]
    fn counts_and_times_are_folded_from_the_whole_history() {
        let got = stats(&timed());
        assert_eq!(
            (
                got.turns,
                got.steps,
                got.tool_calls,
                got.failed_tool_calls,
                got.context_messages
            ),
            (1, 2, 2, 1, 1)
        );
        assert_eq!(
            got.llm_ms,
            500 + 500,
            "100->1600 is not counted from the person's message but from the last thing sent"
        );
        assert_eq!(got.tool_ms, 400, "one batch, counted once");
    }

    #[test]
    fn first_token_and_decode_times_are_summed_over_the_replies_that_have_them() {
        let timing = |first, decode| vivianne_core::Timing {
            first_token_ms: first,
            decode_ms: decode,
        };
        let mut history = timed();
        history[2] = history[2].clone().timed(timing(300, 200));
        history[5] = history[5].clone().timed(timing(100, 50));
        let got = stats(&history);
        assert_eq!(
            (got.first_token_ms, got.decode_ms, got.timed_steps),
            (400, 250, 2)
        );
        // A reply from before timings were kept counts as a step and adds nothing to them.
        let older = stats(&timed());
        assert_eq!(
            (
                older.steps,
                older.first_token_ms,
                older.decode_ms,
                older.timed_steps
            ),
            (2, 0, 0, 0)
        );
    }

    #[test]
    fn a_history_without_times_still_counts() {
        let history = vec![
            Message::new(Role::User, "q"),
            Message::assistant("a".to_owned(), Vec::new()),
        ];
        assert_eq!(
            stats(&history),
            Stats {
                turns: 1,
                steps: 1,
                ..Stats::default()
            }
        );
        assert_eq!(stats(&[]), Stats::default());
    }

    #[test]
    fn a_turn_nobody_answered_is_not_counted() {
        let history = vec![
            Message::new(Role::User, "q1"),
            Message::assistant("a".to_owned(), Vec::new()),
            Message::new(Role::User, "q2"),
        ];
        assert_eq!(stats(&history).turns, 1);
    }

    #[test]
    fn the_outline_has_one_entry_per_turn_with_bounded_one_line_previews() {
        let mut history = timed();
        history.push(Message::assistant(
            "a   long\nanswer ".repeat(40),
            Vec::new(),
        ));
        let got = outline(&history);
        assert_eq!(got.len(), 2);
        assert_eq!(
            (
                got[0].turn,
                got[0].seq,
                got[0].prompt.as_str(),
                got[0].response.as_str()
            ),
            (1, 0, "first question", "the answer")
        );
        assert_eq!(
            (got[1].turn, got[1].seq, got[1].prompt.as_str()),
            (2, 6, "second")
        );
        assert!(
            got[1].response.chars().count() <= RESPONSE_PREVIEW
                && got[1].response.ends_with('…')
                && !got[1].response.contains('\n')
        );
    }

    #[test]
    fn a_long_prompt_is_clipped_and_a_condensed_checkpoint_in_front_is_not_shown() {
        let history = vec![Message::new(
            Role::User,
            format!(
                "<compacted_history>\nsummary of the old part\n</compacted_history>\n\n{}",
                "word ".repeat(30)
            ),
        )];
        let got = outline(&history);
        assert!(
            got[0].prompt.starts_with("word word")
                && !got[0].prompt.contains("summary")
                && got[0].prompt.chars().count() <= PROMPT_PREVIEW,
            "{:?}",
            got[0].prompt
        );
        assert_eq!(
            outline(&[Message::new(
                Role::User,
                "<compacted_history>only a summary</compacted_history>"
            )])[0]
                .prompt,
            ""
        );
    }

    #[test]
    fn previews_collapse_whitespace_and_clip_by_characters_not_bytes() {
        assert_eq!(preview("  a \n b\t c  ", 20), "a b c");
        assert_eq!(preview("héllo wörld", 8), "héllo w…");
        assert_eq!(
            preview("abc", 3),
            "ab…",
            "the ellipsis counts toward the limit"
        );
        assert_eq!(preview("", 5), "");
    }

    #[test]
    fn the_messages_the_app_added_are_not_turns_and_never_the_response() {
        let history = vec![
            Message::new(Role::User, "q"),
            Message::context("clock", ContextSource::TimeContext),
            Message::assistant("final".to_owned(), Vec::new()),
        ];
        let got = outline(&history);
        assert_eq!((got.len(), got[0].response.as_str()), (1, "final"));
    }
}
