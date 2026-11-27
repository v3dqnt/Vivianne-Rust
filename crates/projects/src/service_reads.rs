//! The lists, each behind its own permission and each bounded.

use crate::audit;
use crate::join::JoinRequest;
use crate::membership::Membership;
use crate::model::{Project, ProjectId, UserId};
use crate::permission::Permission;
use crate::service::{Call, Error, Projects};
use crate::store::{MAX_PAGE, Store};

impl<S: Store> Projects<'_, S> {
    /// Public, active projects. Nothing private appears here, and no sign-in is needed.
    pub fn directory(&self, after: Option<&str>, limit: usize) -> Result<Vec<Project>, Error> {
        Ok(self.store.public_projects(after, limit.min(MAX_PAGE))?)
    }

    pub fn members(
        &self,
        call: &Call,
        id: &ProjectId,
        after: Option<&UserId>,
    ) -> Result<Vec<Membership>, Error> {
        self.allowed(call, id, Permission::ViewContent)?;
        Ok(self.store.members(id, after, MAX_PAGE)?)
    }

    /// Requests waiting for a decision, for those who may decide them.
    pub fn waiting_requests(&self, call: &Call, id: &ProjectId) -> Result<Vec<JoinRequest>, Error> {
        self.allowed(call, id, Permission::ReviewJoinRequests)?;
        Ok(self.store.pending_joins(id, call.now, MAX_PAGE)?)
    }

    /// The audit trail is for those who manage the project, not every member.
    pub fn audit_log(&self, call: &Call, id: &ProjectId) -> Result<Vec<audit::Event>, Error> {
        self.allowed(call, id, Permission::ManagePolicy)?;
        Ok(self.store.audit(id, MAX_PAGE)?)
    }
}
