//! The filter vocabulary. Filters are ANDed; the values inside one clause are ORed; an empty list matches nothing;
//! ranges are inclusive. A range with `from` past `to`, a text clause with nothing in it and a malformed cursor are
//! refused, never read as "everything".

use serde::{Deserialize, Serialize};

use super::{EventRecord, QueryError, ThreadRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Range {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

impl Range {
    pub fn check(self, what: &str) -> Result<Self, QueryError> {
        if let (Some(from), Some(to)) = (self.from, self.to)
            && from > to
        {
            return Err(QueryError::InvalidFilter(format!(
                "{what} range: from must not be past to"
            )));
        }
        Ok(self)
    }

    #[must_use]
    pub fn holds(self, value: u64) -> bool {
        self.from.is_none_or(|f| value >= f) && self.to.is_none_or(|t| value <= t)
    }
}

/// What kind of event something is. `Context` is what the app added for the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    User,
    Context,
    Assistant,
    ToolResult,
    Checkpoint,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Context => "context",
            Self::Assistant => "assistant",
            Self::ToolResult => "tool-result",
            Self::Checkpoint => "checkpoint",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        [
            Self::User,
            Self::Context,
            Self::Assistant,
            Self::ToolResult,
            Self::Checkpoint,
        ]
        .into_iter()
        .find(|k| k.as_str() == text)
    }
}

/// Whether an event is in the conversation as it stands, or only in an earlier state that was condensed or rewound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    Current,
    Shadowed,
}

impl Surface {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Shadowed => "shadowed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadFilter {
    Id(Vec<String>),
    Workspace(Vec<String>),
    /// Seconds since 1970, as a thread's `updated_at` is.
    UpdatedAt(Range),
    /// The thread it was branched from; `None` matches threads that are not branches.
    Parent(Vec<Option<String>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventFilter {
    Seq(Range),
    /// Milliseconds since 1970. An event with no recorded time never matches.
    Time(Range),
    Kind(Vec<Kind>),
    Surface(Vec<Surface>),
    /// A literal, case-insensitive match; runs of white space count as one.
    Text(String),
}

/// The filters that can be answered from an event's record alone (the ones a search accepts beside its text).
pub type MetadataFilter = EventFilter;

pub fn check_thread_filters(filters: &[ThreadFilter]) -> Result<(), QueryError> {
    for filter in filters {
        if let ThreadFilter::UpdatedAt(range) = filter {
            range.check("updated-at")?;
        }
    }
    Ok(())
}

pub fn check_event_filters(filters: &[EventFilter]) -> Result<(), QueryError> {
    for filter in filters {
        match filter {
            EventFilter::Seq(range) => {
                range.check("seq")?;
            }
            EventFilter::Time(range) => {
                range.check("time")?;
            }
            EventFilter::Text(text) if text.trim().is_empty() => {
                return Err(QueryError::InvalidFilter(
                    "a text filter must contain something other than white space".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[must_use]
pub fn thread_matches(record: &ThreadRecord, filters: &[ThreadFilter]) -> bool {
    filters.iter().all(|filter| match filter {
        ThreadFilter::Id(ids) => ids.contains(&record.id),
        ThreadFilter::Workspace(spaces) => spaces.contains(&record.workspace),
        ThreadFilter::UpdatedAt(range) => range.holds(record.updated_at),
        ThreadFilter::Parent(parents) => {
            parents.contains(&record.parent.as_ref().map(|p| p.thread.clone()))
        }
    })
}

/// `text` lower-cased with every run of white space made one space, which is what a literal match compares.
fn flat(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `text` holds `wanted` as a literal, ignoring case and the width of the white space between words.
#[must_use]
pub fn text_matches(text: &str, wanted: &str) -> bool {
    flat(text).contains(&flat(wanted))
}

#[must_use]
pub fn event_matches(record: &EventRecord, text: &str, filters: &[EventFilter]) -> bool {
    filters.iter().all(|filter| match filter {
        EventFilter::Seq(range) => range.holds(record.seq as u64),
        EventFilter::Time(range) => record.time.is_some_and(|t| range.holds(t)),
        EventFilter::Kind(kinds) => kinds.contains(&record.kind),
        EventFilter::Surface(surfaces) => surfaces.contains(&record.surface),
        EventFilter::Text(wanted) => text_matches(text, wanted),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_match_is_literal_case_blind_and_ignores_the_width_of_white_space() {
        assert!(text_matches("Fix the  Parser\n\nnow", "fix the parser"));
        assert!(
            text_matches("a.b*c", "A.B*C"),
            "no pattern characters have a meaning"
        );
        assert!(!text_matches("fix parser", "fix the parser"));
        assert!(text_matches("ÉCOLE", "école"));
    }

    #[test]
    fn a_backwards_range_and_an_empty_text_are_refused_not_read_as_everything() {
        let backwards = Range {
            from: Some(5),
            to: Some(1),
        };
        assert!(check_event_filters(&[EventFilter::Seq(backwards)]).is_err());
        assert!(check_thread_filters(&[ThreadFilter::UpdatedAt(backwards)]).is_err());
        assert!(check_event_filters(&[EventFilter::Text("  \n".into())]).is_err());
        assert!(
            check_event_filters(&[EventFilter::Seq(Range {
                from: Some(1),
                to: Some(1)
            })])
            .is_ok()
        );
    }

    #[test]
    fn ranges_include_their_ends_and_an_empty_list_matches_nothing() {
        let range = Range {
            from: Some(2),
            to: Some(4),
        };
        assert!([1, 2, 4, 5].map(|v| range.holds(v)) == [false, true, true, false]);
        let record = ThreadRecord {
            id: "a".into(),
            workspace: "/w".into(),
            title: "t".into(),
            updated_at: 3,
            parent: None,
            messages: 0,
            generations: vec![],
        };
        assert!(!thread_matches(&record, &[ThreadFilter::Id(vec![])]));
        assert!(thread_matches(&record, &[ThreadFilter::Parent(vec![None])]));
        assert!(thread_matches(&record, &[]));
    }
}
