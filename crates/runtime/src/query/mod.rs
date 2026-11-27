//! Reading, filtering and tracing saved conversations (the original `session-query`). Reads see the conversation as it
//! is saved: there are no live sessions apart from the saved ones, so there is no live/persisted split. Results are
//! owned copies, and one call reads each thread once.

pub mod extract;
pub mod filters;
pub mod search;
pub mod trace;

use std::path::{Path, PathBuf};

use serde::Serialize;
use vivianne_core::Message;

use crate::log;
use crate::store::{self, Parent, SavedThread};
use filters::{
    EventFilter, Kind, Surface, ThreadFilter, check_event_filters, check_thread_filters,
    event_matches, thread_matches,
};

/// The most events either side of one that a read may ask for.
pub const READ_WINDOW_MAX: usize = 50;

/// Why a query failed. Each says what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("there is no saved thread {0}")]
    NotFound(String),
    #[error("thread {thread} has no event {seq} (it has {events})")]
    NoSuchEvent {
        thread: String,
        seq: usize,
        events: usize,
    },
    #[error("invalid filter: {0}")]
    InvalidFilter(String),
    #[error("invalid cursor")]
    InvalidCursor,
    /// A saved file that cannot be read: damaged, or written by a newer build. It is not touched.
    #[error("thread {thread} cannot be read: {why}")]
    Unreadable { thread: String, why: String },
    #[error("storage: {0}")]
    Storage(String),
}

/// A conversation as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadRecord {
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub updated_at: u64,
    pub parent: Option<Parent>,
    pub messages: usize,
    /// Earlier states kept when the conversation was condensed or rewound, oldest first.
    pub generations: Vec<u32>,
}

/// Where an event sits, without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventRecord {
    pub thread: String,
    /// The state of the conversation it is in; the current one has the highest number.
    pub generation: u32,
    pub seq: usize,
    pub kind: Kind,
    pub time: Option<u64>,
    pub surface: Surface,
}

/// An event record with the text that can be searched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventDoc {
    #[serde(flatten)]
    pub record: EventRecord,
    pub text: String,
}

/// One event, with the ones around it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventWindow {
    pub thread: String,
    pub generation: u32,
    pub target: Message,
    pub start_seq: usize,
    pub end_seq: usize,
    pub events: Vec<Message>,
}

/// How one event relates to others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventLinks {
    pub target: EventRecord,
    #[serde(flatten)]
    pub trace: trace::EventTrace,
    /// For a checkpoint: the earlier state of the conversation it condensed.
    pub replaces_generation: Option<u32>,
    /// For an event in an earlier state: the generation that replaced it.
    pub replaced_by_generation: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Query {
    dir: PathBuf,
}

fn record_of(thread: &SavedThread, dir: &Path) -> ThreadRecord {
    ThreadRecord {
        id: thread.id.clone(),
        workspace: thread.workspace.clone(),
        title: thread.title.clone(),
        updated_at: thread.updated_at,
        parent: thread.parent.clone(),
        messages: thread.history.len(),
        generations: log::generations(dir, &thread.id),
    }
}

fn docs_of(thread: &str, generation: u32, current: bool, history: &[Message]) -> Vec<EventDoc> {
    history
        .iter()
        .enumerate()
        .map(|(seq, message)| EventDoc {
            record: EventRecord {
                thread: thread.to_owned(),
                generation,
                seq,
                kind: extract::kind_of(message),
                time: message.at,
                surface: if current {
                    Surface::Current
                } else {
                    Surface::Shadowed
                },
            },
            text: extract::text_of(message),
        })
        .collect()
}

impl Query {
    /// Reads from `dir`, a folder of saved threads.
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
        }
    }

    /// Reads from `~/.viv/agent/threads`.
    pub fn standard() -> Result<Self, QueryError> {
        store::threads_dir()
            .map(|dir| Self { dir })
            .map_err(QueryError::Storage)
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn thread(&self, id: &str) -> Result<SavedThread, QueryError> {
        match store::read_thread_in(&self.dir, id) {
            Ok(Some(thread)) => Ok(thread),
            Ok(None) => Err(QueryError::NotFound(id.to_owned())),
            Err(why) => Err(QueryError::Unreadable {
                thread: id.to_owned(),
                why,
            }),
        }
    }

    /// Every readable thread, newest first. A file that cannot be read is left out (`store::problems` names those).
    #[must_use]
    pub fn list_threads(&self) -> Vec<ThreadRecord> {
        let mut found: Vec<ThreadRecord> = store::all_in(&self.dir)
            .iter()
            .map(|t| record_of(t, &self.dir))
            .collect();
        found.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        found
    }

    pub fn filter_threads(
        &self,
        filters: &[ThreadFilter],
    ) -> Result<Vec<ThreadRecord>, QueryError> {
        check_thread_filters(filters)?;
        Ok(self
            .list_threads()
            .into_iter()
            .filter(|r| thread_matches(r, filters))
            .collect())
    }

    /// The whole saved conversation as it stands.
    pub fn read_thread(&self, id: &str) -> Result<SavedThread, QueryError> {
        self.thread(id)
    }

    /// Every event of the conversation as it stands, and of each earlier state, with its text.
    pub fn documents(&self, id: &str) -> Result<Vec<EventDoc>, QueryError> {
        let thread = self.thread(id)?;
        Ok(self.documents_of(&thread))
    }

    pub(crate) fn documents_of(&self, thread: &SavedThread) -> Vec<EventDoc> {
        let current = store::generation_in(&self.dir, &thread.id);
        let mut out = Vec::new();
        for generation in log::generations(&self.dir, &thread.id) {
            if let Ok(Some(old)) = log::read_generation(&self.dir, &thread.id, generation) {
                out.extend(docs_of(&thread.id, generation, false, &old.history));
            }
        }
        out.extend(docs_of(&thread.id, current, true, &thread.history));
        out
    }

    /// Where each event is, without its text.
    pub fn list_events(&self, id: &str) -> Result<Vec<EventRecord>, QueryError> {
        Ok(self.documents(id)?.into_iter().map(|d| d.record).collect())
    }

    /// The events that pass every filter, with their text.
    pub fn filter_events(
        &self,
        id: &str,
        filters: &[EventFilter],
    ) -> Result<Vec<EventDoc>, QueryError> {
        check_event_filters(filters)?;
        Ok(self
            .documents(id)?
            .into_iter()
            .filter(|d| event_matches(&d.record, &d.text, filters))
            .collect())
    }

    /// One event of the conversation as it stands, with up to `before` and `after` neighbours (each at most
    /// [`READ_WINDOW_MAX`]).
    pub fn read_event(
        &self,
        id: &str,
        seq: usize,
        before: usize,
        after: usize,
    ) -> Result<EventWindow, QueryError> {
        if before > READ_WINDOW_MAX || after > READ_WINDOW_MAX {
            return Err(QueryError::InvalidFilter(format!(
                "a window is at most {READ_WINDOW_MAX} events either side"
            )));
        }
        let thread = self.thread(id)?;
        let target = thread
            .history
            .get(seq)
            .cloned()
            .ok_or(QueryError::NoSuchEvent {
                thread: id.to_owned(),
                seq,
                events: thread.history.len(),
            })?;
        let start = seq.saturating_sub(before);
        let end = (seq + after).min(thread.history.len() - 1);
        Ok(EventWindow {
            thread: id.to_owned(),
            generation: store::generation_in(&self.dir, id),
            target,
            start_seq: start,
            end_seq: end,
            events: thread.history[start..=end].to_vec(),
        })
    }

    /// The conversations this one came from and the ones that came from it.
    pub fn trace_thread(&self, id: &str) -> Result<trace::Lineage, QueryError> {
        let all = self.list_threads();
        let target = all
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| QueryError::NotFound(id.to_owned()))?;
        Ok(trace::lineage(&target, &all))
    }

    /// How the event at `seq` relates to the others around it.
    pub fn trace_event(&self, id: &str, seq: usize) -> Result<EventLinks, QueryError> {
        let thread = self.thread(id)?;
        let generation = store::generation_in(&self.dir, id);
        let history = &thread.history;
        let links = trace::event_trace(history, seq).ok_or(QueryError::NoSuchEvent {
            thread: id.to_owned(),
            seq,
            events: history.len(),
        })?;
        let kind = extract::kind_of(&history[seq]);
        Ok(EventLinks {
            target: EventRecord {
                thread: id.to_owned(),
                generation,
                seq,
                kind,
                time: history[seq].at,
                surface: Surface::Current,
            },
            trace: links,
            replaces_generation: (kind == Kind::Checkpoint && generation > 1)
                .then(|| generation - 1),
            replaced_by_generation: None,
        })
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Helpers other modules' tests use.
#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) use super::tests::{dir, thread};
}
