//! Mentioning other conversations of the same project. The rules (the `viv-session:` link, what is quoted, the byte
//! budget) are in `vivianne-context`; this puts them over the saved threads. A message that holds a mention is sent with
//! the mention turned into plain `@title` text and a second, hidden message that carries an untrusted, read-only snapshot
//! of what the other conversation said.

use vivianne_context::session_ref::{
    Candidate, Config, Header, SessionReferenceError, SessionSource, Snapshot, list_candidates,
    prepare,
};
use vivianne_core::Message;

use crate::store;

/// The saved threads of one project, as sessions that may be mentioned.
#[derive(Debug, Clone)]
pub struct ProjectThreads {
    workspace: String,
}

impl ProjectThreads {
    #[must_use]
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_owned(),
        }
    }

    fn headers(&self) -> Vec<Header> {
        store::list(&self.workspace)
            .into_iter()
            .map(|summary| Header {
                id: summary.id,
                label: summary.title,
                cwd: Some(self.workspace.clone()),
                created_at: summary.updated_at,
            })
            .collect()
    }
}

impl SessionSource for ProjectThreads {
    fn list(&self) -> Result<Vec<Header>, String> {
        Ok(self.headers())
    }

    fn read(&self, id: &str) -> Result<Snapshot, String> {
        // Only this project's conversations: an id from another project reads as if there were none.
        let thread = store::load(id)
            .filter(|t| t.workspace == self.workspace)
            .ok_or_else(|| format!("there is no saved conversation {id} in this project"))?;
        Ok(Snapshot {
            header: Header {
                id: thread.id,
                label: thread.title,
                cwd: Some(thread.workspace),
                created_at: thread.updated_at,
            },
            messages: thread.history,
        })
    }
}

/// The conversations that may be mentioned from `thread` and match `query`, the ones of this project first.
pub fn candidates(
    thread: &str,
    workspace: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<Candidate>, SessionReferenceError> {
    list_candidates(
        &ProjectThreads::new(workspace).headers(),
        thread,
        Some(workspace),
        query,
        limit,
    )
}

/// The text that mentions `id`, ready to put in a message.
#[must_use]
pub fn mention_of(id: &str, workspace: &str) -> Option<String> {
    let found = ProjectThreads::new(workspace)
        .headers()
        .into_iter()
        .find(|h| h.id == id)?;
    Some(vivianne_context::session_ref::format_mention(
        &found.id,
        Some(&found.label),
    ))
}

/// Turns the mentions in a message into plain text and a snapshot message. A message without a mention comes back as it
/// was, with no snapshot. An error says why a mention could not be read, in words for the person.
pub fn prepare_message(
    thread: &str,
    workspace: &str,
    text: &str,
    context_window: Option<u64>,
) -> Result<(String, Option<Message>), String> {
    if !text.contains(vivianne_context::session_ref::uri::SCHEME) {
        return Ok((text.to_owned(), None));
    }
    let prepared = prepare(
        &Config::default(),
        thread,
        text,
        &ProjectThreads::new(workspace),
        None,
        context_window,
        &|| false,
    )
    .map_err(|e| e.to_string())?;
    Ok((prepared.text, prepared.additional))
}

#[cfg(test)]
mod tests {
    use vivianne_core::Role;

    use super::*;

    #[test]
    fn a_message_without_a_mention_is_left_alone() {
        let (text, extra) = prepare_message("me", "/w", "just a question", None).expect("fine");
        assert_eq!((text.as_str(), extra.is_none()), ("just a question", true));
    }

    #[test]
    fn a_mention_of_a_conversation_that_is_not_there_says_so() {
        let mention = vivianne_context::session_ref::format_mention(
            "11111111-1111-1111-1111-111111111111",
            Some("gone"),
        );
        let error = prepare_message("me", "/w", &format!("see {mention}"), None).unwrap_err();
        assert!(
            error.contains("no saved conversation") || error.contains("read"),
            "{error}"
        );
    }

    #[test]
    fn a_broken_link_is_refused_not_guessed() {
        assert!(prepare_message("me", "/w", "@[x](viv-session:!!!)", None).is_err());
    }

    #[test]
    fn a_conversation_cannot_mention_itself() {
        let mention = vivianne_context::session_ref::format_mention("me", None);
        assert!(prepare_message("me", "/w", &mention, None).is_err());
        let _ = Role::User;
    }
}
