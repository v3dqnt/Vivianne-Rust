//! The records a project is made of.

use serde::{Deserialize, Serialize};

/// The format of a saved project record; a higher number is a newer build's and is not rewritten (R3.5).
pub const FORMAT: u32 = 1;

pub const MAX_NAME: usize = 100;
pub const MAX_SUMMARY: usize = 500;

/// A Vivianne user. It must come from an authenticated session, never from request data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub String);

impl From<&str> for UserId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<&str> for ProjectId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Whether the listing and profile can be discovered. It says nothing about the repository, threads or traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    #[default]
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Draft,
    Active,
    Archived,
    Suspended,
    DeletionPending,
    Deleted,
}

impl Lifecycle {
    /// Whether a project may move from this state to `to`. `Deleted` is final.
    #[must_use]
    pub fn can_become(self, to: Self) -> bool {
        use Lifecycle::{Active, Archived, Deleted, DeletionPending, Draft, Suspended};
        matches!(
            (self, to),
            (Draft, Active | DeletionPending)
                | (Active, Archived | Suspended | DeletionPending)
                | (Archived, Active | Suspended | DeletionPending)
                | (Suspended, Active | Archived | DeletionPending)
                | (DeletionPending, Archived | Deleted)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinPolicy {
    /// A request is approved as it is made.
    Open,
    #[default]
    Approval,
    InviteOnly,
    Closed,
}

/// What a project narrows or opens within the role table. It never goes past the platform's limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent switches a person sets"
)]
pub struct Policy {
    pub maintainers_review_requests: bool,
    pub maintainers_invite: bool,
    pub contributors_run_agents: bool,
    pub public_threads_allowed: bool,
    pub rejection_cooldown_ms: u64,
    pub request_lifetime_ms: u64,
    pub max_members: usize,
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

impl Default for Policy {
    fn default() -> Self {
        Self {
            maintainers_review_requests: false,
            maintainers_invite: false,
            contributors_run_agents: true,
            public_threads_allowed: false,
            rejection_cooldown_ms: 7 * DAY_MS,
            request_lifetime_ms: 30 * DAY_MS,
            max_members: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub format: u32,
    pub id: ProjectId,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub visibility: Visibility,
    pub lifecycle: Lifecycle,
    pub join_policy: JoinPolicy,
    pub policy: Policy,
    /// Raised when the policy or visibility changes; runs are pinned to it.
    pub policy_version: u32,
    /// For optimistic concurrency: a write names the revision it follows.
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error(
        "a slug is 3 to 48 lower-case letters, digits or hyphens, not starting or ending with a hyphen"
    )]
    Slug,
    #[error("a name is 1 to {MAX_NAME} characters with no control characters")]
    Name,
    #[error(
        "a summary is at most {MAX_SUMMARY} characters, with no control characters but new lines"
    )]
    Summary,
}

/// Checks the text a person typed before it becomes a public listing.
pub fn check_listing(slug: &str, name: &str, summary: &str) -> Result<(), Invalid> {
    let slug_ok = (3..=48).contains(&slug.len())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if !slug_ok {
        return Err(Invalid::Slug);
    }
    let named = !name.trim().is_empty() && name.chars().count() <= MAX_NAME;
    if !named || name.chars().any(char::is_control) {
        return Err(Invalid::Name);
    }
    if summary.chars().count() > MAX_SUMMARY || has_controls(summary) {
        return Err(Invalid::Summary);
    }
    Ok(())
}

/// Control characters other than new lines, which would act on a terminal that printed them (R4.6).
#[must_use]
pub fn has_controls(text: &str) -> bool {
    text.chars().any(|c| c.is_control() && c != '\n')
}

/// Who a thread, trace, log or artifact is shared with. Private to its creator unless shared on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Share {
    #[default]
    Creator,
    Members(Vec<UserId>),
    AllMembers,
    Public,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_is_final_and_a_draft_cannot_be_archived() {
        use Lifecycle::{Active, Archived, Deleted, DeletionPending, Draft, Suspended};
        for to in [Draft, Active, Archived, Suspended, DeletionPending] {
            assert!(!Deleted.can_become(to), "{to:?}");
        }
        assert!(!Draft.can_become(Archived));
        assert!(!Active.can_become(Deleted), "deletion goes through pending");
        assert!(Draft.can_become(Active) && Active.can_become(Archived));
    }

    #[test]
    fn hostile_listing_text_is_refused() {
        assert_eq!(check_listing("ok-slug", "Name", ""), Ok(()));
        for slug in [
            "",
            "ab",
            "-abc",
            "abc-",
            "Abc",
            "a/b/c",
            "../etc",
            &"a".repeat(49),
        ] {
            assert_eq!(check_listing(slug, "n", ""), Err(Invalid::Slug), "{slug}");
        }
        assert_eq!(check_listing("abc", "  ", ""), Err(Invalid::Name));
        assert_eq!(check_listing("abc", "a\u{1b}[2J", ""), Err(Invalid::Name));
        assert_eq!(
            check_listing("abc", "n", &"x".repeat(501)),
            Err(Invalid::Summary)
        );
    }

    #[test]
    fn a_summary_may_have_new_lines_but_no_terminal_escapes() {
        assert_eq!(
            check_listing("abc", "n", "a\u{1b}]0;x"),
            Err(Invalid::Summary)
        );
        assert_eq!(check_listing("abc", "n", "two\nlines"), Ok(()));
    }

    #[test]
    fn nothing_is_public_unless_made_so() {
        assert_eq!(Visibility::default(), Visibility::Private);
        assert_eq!(Share::default(), Share::Creator);
    }
}
