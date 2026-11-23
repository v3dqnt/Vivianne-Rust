//! Where a durable context message came from. Workspace instructions, the clock, the terminal's location and
//! referenced sessions all enter a conversation as ordinary user-role messages, so they are saved, replayed and
//! condensed like any other history. A message that carries a `ContextSource` was added by the app, not typed by the
//! person: the transcript does not show it as the person's words, a title is never made from it, and the code that
//! produced it finds its own earlier output by this tag.

use serde::{Deserialize, Serialize};

/// What happened to one instruction file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeAction {
    /// First time this file is shown.
    Set,
    /// Its content changed since it was shown.
    Replace,
    /// It is gone, or now repeats an earlier file.
    Remove,
}

/// One transition of one instruction file, as recorded in the message that announced it. The latest change of each scope is
/// the state the model is working from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionChange {
    pub action: ChangeAction,
    /// Which candidate file: its directory and name, as `scope_key` builds it.
    pub scope: String,
    /// The path the model was shown.
    pub path: String,
    /// A digest of the content shown (absent for a removal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Instruction files announced by one message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionsSource {
    /// The complete startup (or resume) set, as against a later change.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baseline: bool,
    /// What the discovery, precedence and budget were, so a resumed baseline can be checked against today's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_identity: Option<String>,
    pub changes: Vec<InstructionChange>,
}

/// Facts about one session quoted into a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceFacts {
    pub session_id: String,
    pub label: String,
    /// How many messages the source had when it was captured.
    pub captured_messages: usize,
    pub compacted: bool,
    pub original_messages: usize,
    pub retained_messages: usize,
    pub omitted_messages: usize,
    pub omitted_bytes: usize,
    pub truncated: bool,
    pub input_index: usize,
}

/// Referenced sessions quoted into a message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReferenceSource {
    pub references: Vec<ReferenceFacts>,
}

/// Who added a context message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ContextSource {
    AgentInstructions(InstructionsSource),
    TimeContext,
    TmuxContext,
    SessionReference(SessionReferenceSource),
}

impl ContextSource {
    /// The instruction files this message announced, if it is an instruction message.
    #[must_use]
    pub fn instructions(&self) -> Option<&InstructionsSource> {
        match self {
            Self::AgentInstructions(i) => Some(i),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_round_trips_and_leaves_out_what_is_empty() {
        let source = ContextSource::AgentInstructions(InstructionsSource {
            baseline: true,
            baseline_identity: Some("id".into()),
            changes: vec![InstructionChange {
                action: ChangeAction::Set,
                scope: "a".into(),
                path: "AGENTS.md".into(),
                digest: Some("d".into()),
            }],
        });
        let json = serde_json::to_string(&source).expect("serializes");
        assert!(
            json.contains(r#""kind":"agent-instructions""#) && json.contains(r#""action":"set""#)
        );
        assert_eq!(
            serde_json::from_str::<ContextSource>(&json).expect("reads"),
            source
        );
        let later = ContextSource::AgentInstructions(InstructionsSource::default());
        assert!(
            !serde_json::to_string(&later)
                .expect("serializes")
                .contains("baseline")
        );
        assert_eq!(
            serde_json::to_string(&ContextSource::TimeContext).expect("serializes"),
            r#"{"kind":"time-context"}"#
        );
    }

    #[test]
    fn an_unknown_kind_is_not_guessed_and_does_not_stop_a_thread_opening() {
        assert!(serde_json::from_str::<ContextSource>(r#"{"kind":"from-the-future"}"#).is_err());
        let message: crate::Message = serde_json::from_str(
            r#"{"role":"user","text":"x","context":{"kind":"from-the-future"}}"#,
        )
        .expect("opens");
        assert!(message.context.is_none() && message.is_typed());
    }

    #[test]
    fn a_context_message_is_not_typed_and_keeps_its_time() {
        let m = crate::Message::context("t", ContextSource::TimeContext).at(5);
        assert!(!m.is_typed() && m.at == Some(5));
        let json = serde_json::to_string(&m).expect("serializes");
        assert_eq!(
            serde_json::from_str::<crate::Message>(&json).expect("reads"),
            m
        );
    }
}
