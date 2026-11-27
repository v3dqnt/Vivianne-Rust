//! The text of an event that is worth searching. What the app added for the model (instruction files, the clock) and
//! structural events are left out, so a search for "tests" finds what was said about tests and not the instructions
//! every conversation begins with. Reasoning is never kept in a thread, so there is none to leave out.

use vivianne_core::{Message, Role};

use super::filters::Kind;

/// What kind of event a message is. A typed message that carries a condensed history is a checkpoint.
#[must_use]
pub fn kind_of(message: &Message) -> Kind {
    match message.role {
        Role::User if message.context.is_some() => Kind::Context,
        Role::User if message.text.trim_start().starts_with("<compacted_history>") => {
            Kind::Checkpoint
        }
        Role::User => Kind::User,
        Role::Assistant => Kind::Assistant,
        Role::Tool => Kind::ToolResult,
        Role::System => Kind::Context,
    }
}

/// The searchable text of a message, or an empty string when there is none.
#[must_use]
pub fn text_of(message: &Message) -> String {
    match kind_of(message) {
        Kind::Context | Kind::Checkpoint => String::new(),
        Kind::User | Kind::ToolResult => message.text.trim().to_owned(),
        Kind::Assistant => {
            let mut parts = vec![message.text.trim().to_owned()];
            for call in &message.tool_calls {
                parts.push(call.name.clone());
                parts.push(call.args.to_string());
            }
            parts.retain(|p| !p.is_empty());
            parts.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;
    use vivianne_core::context::ContextSource;

    use super::*;

    #[test]
    fn what_the_app_added_and_condensed_history_are_not_searchable() {
        let context = Message::context(
            "<system-reminder>x</system-reminder>",
            ContextSource::TimeContext,
        );
        let checkpoint = Message::new(Role::User, "<compacted_history>old</compacted_history>");
        assert_eq!(
            (kind_of(&context), text_of(&context)),
            (Kind::Context, String::new())
        );
        assert_eq!(
            (kind_of(&checkpoint), text_of(&checkpoint)),
            (Kind::Checkpoint, String::new())
        );
    }

    #[test]
    fn a_reply_is_its_words_and_the_tools_it_called_with_their_arguments() {
        let call = ToolCall {
            id: "c".into(),
            name: "read".into(),
            args: json!({ "path": "a.rs" }),
        };
        let reply = Message::assistant("looking".to_owned(), vec![call]);
        let text = text_of(&reply);
        assert!(
            text.contains("looking") && text.contains("read") && text.contains("a.rs"),
            "{text}"
        );
        assert_eq!(text_of(&Message::new(Role::User, "  hi \n")), "hi");
        assert_eq!(text_of(&Message::assistant(String::new(), vec![])), "");
    }
}
