//! The boundary a repository host is behind (§6.1). Project and membership code never names a particular host;
//! Phase 2 adds the first adapter and Phase 3 the workspace and change operations.

use async_trait::async_trait;

use crate::sync::{Access, Failure};

/// What an adapter can do, stated rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent capabilities of a host"
)]
pub struct Supports {
    pub manage_collaborators: bool,
    pub open_pull_requests: bool,
    pub merge: bool,
    pub large_files: bool,
}

/// A repository as the host reports it. `id` is the host's stable id, not a URL, and never carries credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: String,
    pub display_name: String,
    pub default_branch: Option<String>,
    pub archived: bool,
}

#[async_trait]
pub trait RepositoryProvider: Send + Sync {
    fn supports(&self) -> Supports;

    async fn inspect(&self, repository: &str) -> Result<Repository, Failure>;

    /// Give an external identity access. Safe to repeat with the same `idempotency_key`.
    async fn grant(
        &self,
        repository: &str,
        identity: &str,
        idempotency_key: &str,
    ) -> Result<Access, Failure>;

    async fn revoke(
        &self,
        repository: &str,
        identity: &str,
        idempotency_key: &str,
    ) -> Result<Access, Failure>;

    /// What access the host says the identity has now; used to reconcile.
    async fn access(&self, repository: &str, identity: &str) -> Result<Access, Failure>;
}
