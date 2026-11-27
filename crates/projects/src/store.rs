//! Where records are kept. A write is one batch that lands whole or not at all, and each record names the revision
//! it follows, so two writers cannot both win. Reads can fail and say so: a failed read is never "no such thing",
//! which would let a block or a suspension pass unseen (R4.2). Lists are bounded (R3.7).

use crate::audit;
use crate::join::JoinRequest;
use crate::membership::{Membership, Transfer};
use crate::model::{Project, ProjectId, UserId};

/// The most any one list returns.
pub const MAX_PAGE: usize = 100;

/// One record to write. A new record has revision 1; a change has the stored revision plus one.
#[derive(Debug, Clone)]
pub enum Write {
    Project(Project),
    Member(Membership),
    Join(JoinRequest),
    Transfer(ProjectId, Option<Transfer>),
    Block(ProjectId, UserId),
    /// Remember what an idempotency key made. A key already used by this person is a conflict.
    Key {
        user: UserId,
        key: String,
        resource: String,
    },
}

/// What must still be true when a batch lands: whatever the decision to write it was read from.
#[derive(Debug, Clone)]
pub enum Guard {
    Project(ProjectId, u64),
    Member(ProjectId, UserId, u64),
    /// How many memberships that are not removed the project has.
    Members(ProjectId, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("someone else changed this first; read it again")]
    Conflict,
    #[error("that slug is taken")]
    SlugTaken,
    #[error("project storage is unavailable")]
    Unavailable,
}

pub type Stored<T> = Result<T, StoreError>;

pub trait Store: Send + Sync {
    fn project(&self, id: &ProjectId) -> Stored<Option<Project>>;
    /// Public, active projects ordered by slug, starting after `after`.
    fn public_projects(&self, after: Option<&str>, limit: usize) -> Stored<Vec<Project>>;
    fn member(&self, project: &ProjectId, user: &UserId) -> Stored<Option<Membership>>;
    /// Memberships that are not removed.
    fn member_count(&self, project: &ProjectId) -> Stored<usize>;
    /// Memberships ordered by user, starting after `after`.
    fn members(
        &self,
        project: &ProjectId,
        after: Option<&UserId>,
        limit: usize,
    ) -> Stored<Vec<Membership>>;
    fn join(&self, id: &str) -> Stored<Option<JoinRequest>>;
    /// One person's requests for one project.
    fn joins_of(&self, project: &ProjectId, user: &UserId) -> Stored<Vec<JoinRequest>>;
    /// Requests still waiting at `now` (pending and not expired), oldest first. An expired one is not waiting,
    /// whether or not anything has marked it.
    fn pending_joins(
        &self,
        project: &ProjectId,
        now: u64,
        limit: usize,
    ) -> Stored<Vec<JoinRequest>>;
    fn transfer(&self, project: &ProjectId) -> Stored<Option<Transfer>>;
    fn is_blocked(&self, project: &ProjectId, user: &UserId) -> Stored<bool>;
    /// The newest events, oldest of them first.
    fn audit(&self, project: &ProjectId, limit: usize) -> Stored<Vec<audit::Event>>;
    /// What an idempotency key made, if this person used it before.
    fn key(&self, user: &UserId, key: &str) -> Stored<Option<String>>;
    /// If every guard still holds, apply every write and append the events; otherwise change nothing.
    fn commit(
        &self,
        guards: Vec<Guard>,
        writes: Vec<Write>,
        events: Vec<audit::Event>,
    ) -> Stored<()>;
}
