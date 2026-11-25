//! A conversation as a typed, numbered sequence of events, and forking it at a turn. Port of the original `session` log's
//! vocabulary on top of what a thread actually keeps: the messages. Nothing here is stored separately; the events are
//! read from the history, so they can never disagree with it.
//!
//! A turn is one thing the person typed and everything that answered it. What the app added for the model (instruction
//! files, the clock) belongs to the turn it was added in, and what a checkpoint condensed is its own kind of event.

use serde::Serialize;
use vivianne_core::{Message, Role};

/// One thing that happened, with its position (`seq`, from 0) and the turn it belongs to (from 1; 0 before the first
/// typed message).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    /// The person typed this.
    User {
        seq: usize,
        turn: usize,
        text: String,
    },
    /// The app added this for the model (`kind` names whose it is).
    Context {
        seq: usize,
        turn: usize,
        kind: String,
        text: String,
    },
    /// A model reply, with the tools it called.
    Assistant {
        seq: usize,
        turn: usize,
        text: String,
        calls: Vec<Call>,
    },
    /// A tool's answer to one call.
    ToolResult {
        seq: usize,
        turn: usize,
        call_id: String,
        text: String,
        is_error: bool,
    },
    /// An earlier part of the conversation, condensed.
    Checkpoint { seq: usize, turn: usize },
}

/// A tool call as the log records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Call {
    pub id: String,
    pub name: String,
}

impl Event {
    #[must_use]
    pub fn seq(&self) -> usize {
        match self {
            Self::User { seq, .. }
            | Self::Context { seq, .. }
            | Self::Assistant { seq, .. }
            | Self::ToolResult { seq, .. }
            | Self::Checkpoint { seq, .. } => *seq,
        }
    }

    #[must_use]
    pub fn turn(&self) -> usize {
        match self {
            Self::User { turn, .. }
            | Self::Context { turn, .. }
            | Self::Assistant { turn, .. }
            | Self::ToolResult { turn, .. }
            | Self::Checkpoint { turn, .. } => *turn,
        }
    }
}

fn kind_of(message: &Message) -> String {
    message
        .context
        .as_ref()
        .and_then(|c| serde_json::to_value(c).ok())
        .and_then(|v| v.get("kind").and_then(|k| k.as_str().map(str::to_owned)))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The history as events. A message that is a condensed checkpoint (a typed one that carries `<compacted_history>`) is
/// reported as a checkpoint followed by what the person typed, if anything is left of it.
#[must_use]
pub fn events(history: &[Message]) -> Vec<Event> {
    let mut out = Vec::with_capacity(history.len());
    let mut turn = 0;
    for message in history {
        let seq = out.len();
        match message.role {
            Role::User if message.context.is_some() => out.push(Event::Context {
                seq,
                turn,
                kind: kind_of(message),
                text: message.text.clone(),
            }),
            Role::User => {
                turn += 1;
                if message.text.trim_start().starts_with("<compacted_history>") {
                    out.push(Event::Checkpoint { seq, turn });
                } else {
                    out.push(Event::User {
                        seq,
                        turn,
                        text: message.text.clone(),
                    });
                }
            }
            Role::Assistant => out.push(Event::Assistant {
                seq,
                turn,
                text: message.text.clone(),
                calls: message
                    .tool_calls
                    .iter()
                    .map(|c| Call {
                        id: c.id.clone(),
                        name: c.name.clone(),
                    })
                    .collect(),
            }),
            Role::Tool => out.push(Event::ToolResult {
                seq,
                turn,
                call_id: message.tool_call_id.clone().unwrap_or_default(),
                text: message.text.clone(),
                is_error: message.text.starts_with("error:"),
            }),
            Role::System => out.push(Event::Context {
                seq,
                turn,
                kind: "system".to_owned(),
                text: message.text.clone(),
            }),
        }
    }
    out
}

/// How many turns the history has: the messages the person typed.
#[must_use]
pub fn turn_count(history: &[Message]) -> usize {
    history.iter().filter(|m| m.is_typed()).count()
}

/// The events from `from` up to (not including) `to`, by position.
#[must_use]
pub fn range(history: &[Message], from: usize, to: usize) -> Vec<Event> {
    events(history)
        .into_iter()
        .filter(|e| e.seq() >= from && e.seq() < to)
        .collect()
}

/// A copy of the conversation as it was before turn `turn` began, to branch from there: every earlier turn whole, with
/// its tool calls answered. `None` for turn 0 or one the conversation never had. Turn 1 gives an empty conversation.
#[must_use]
pub fn fork_before(history: &[Message], turn: usize) -> Option<Vec<Message>> {
    if turn == 0 || turn > turn_count(history) {
        return None;
    }
    let cut = history
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_typed())
        .nth(turn - 1)
        .map(|(i, _)| i)?;
    // What the app added right before the typed message belongs with it (a location reading goes before it).
    let mut cut = cut;
    while cut > 0 && is_pre_turn(&history[cut - 1]) {
        cut -= 1;
    }
    let mut forked = history[..cut].to_vec();
    crate::history::repair(&mut forked);
    Some(forked)
}

/// A reading the loop puts before the person's message rather than after it.
fn is_pre_turn(message: &Message) -> bool {
    matches!(
        message.context,
        Some(vivianne_core::context::ContextSource::TmuxContext)
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;
    use vivianne_core::context::ContextSource;

    use super::*;

    fn conversation() -> Vec<Message> {
        vec![
            Message::new(Role::User, "first question"),
            Message::context(
                "<system-reminder>rules</system-reminder>",
                ContextSource::TimeContext,
            ),
            Message::assistant(
                String::new(),
                vec![ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    args: json!({ "path": "a" }),
                }],
            ),
            Message::tool_result("c1", "file text".to_owned()),
            Message::assistant("answer one".to_owned(), Vec::new()),
            Message::context("tmux", ContextSource::TmuxContext).at(5),
            Message::new(Role::User, "second question"),
            Message::assistant("answer two".to_owned(), Vec::new()),
        ]
    }

    #[test]
    fn the_history_reads_as_numbered_events_with_their_turns() {
        let log = events(&conversation());
        assert_eq!(log.len(), 8);
        assert!(
            matches!(&log[0], Event::User { seq: 0, turn: 1, text } if text == "first question")
        );
        assert!(matches!(&log[1], Event::Context { turn: 1, kind, .. } if kind == "time-context"));
        assert!(matches!(&log[2], Event::Assistant { calls, .. } if calls[0].name == "read"));
        assert!(
            matches!(&log[3], Event::ToolResult { call_id, is_error: false, .. } if call_id == "c1")
        );
        assert!(
            matches!(&log[5], Event::Context { turn: 1, kind, .. } if kind == "tmux-context"),
            "a reading made for turn 2 is still turn 1 until the person types"
        );
        assert!(matches!(&log[6], Event::User { turn: 2, .. }));
        assert_eq!((log[7].seq(), log[7].turn()), (7, 2));
        assert_eq!(turn_count(&conversation()), 2);
    }

    #[test]
    fn a_failed_tool_result_and_a_condensed_head_are_told_apart() {
        let history = vec![
            Message::new(
                Role::User,
                "<compacted_history>\nsummary\n</compacted_history>",
            ),
            Message::assistant(
                String::new(),
                vec![ToolCall {
                    id: "c".into(),
                    name: "shell".into(),
                    args: json!({}),
                }],
            ),
            Message::tool_result("c", "error: exit 1".to_owned()),
        ];
        let log = events(&history);
        assert!(matches!(log[0], Event::Checkpoint { turn: 1, .. }));
        assert!(matches!(&log[2], Event::ToolResult { is_error: true, .. }));
    }

    #[test]
    fn a_range_is_by_position_and_ends_before_its_last_number() {
        let slice = range(&conversation(), 2, 5);
        assert_eq!(slice.iter().map(Event::seq).collect::<Vec<_>>(), [2, 3, 4]);
        assert!(range(&conversation(), 8, 20).is_empty());
    }

    #[test]
    fn forking_before_a_turn_keeps_every_earlier_turn_whole_and_nothing_after() {
        let second = fork_before(&conversation(), 2).expect("a fork");
        assert_eq!(
            second.len(),
            5,
            "the reading made for turn 2 is left behind with it"
        );
        assert_eq!(second.last().map(|m| m.text.as_str()), Some("answer one"));
        assert!(fork_before(&conversation(), 1).expect("a fork").is_empty());
        assert!(
            fork_before(&conversation(), 0).is_none() && fork_before(&conversation(), 3).is_none()
        );
    }

    #[test]
    fn a_fork_never_leaves_a_call_without_its_answer() {
        let mut cut_short = conversation();
        cut_short.remove(3); // the answer to c1 was lost
        let second = fork_before(&cut_short, 2).expect("a fork");
        let answered = second
            .iter()
            .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("c1"));
        assert!(answered, "repair gave the call an answer");
    }
}
