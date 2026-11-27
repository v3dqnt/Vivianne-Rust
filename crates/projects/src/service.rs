//! The operations of Phase 1. Each takes the authenticated actor, loads that actor's stored membership, checks it,
//! and commits the change with its audit event together. A caller never passes a role to be believed, and never
//! chooses an id.
//!
//! The check and the commit are separate steps, so a commit carries guards: the revisions the decision was read
//! from. If one moved, nothing is written and the caller reads again.
//!
//! This file holds the project operations; `service_members.rs` the people and `service_reads.rs` the lists.

use crate::audit::{self, Actor};
use crate::join::{self, JoinError, JoinRequest, MAX_PENDING};
use crate::membership::{Membership, RuleError};
use crate::model::{
    self, FORMAT, Invalid, JoinPolicy, Lifecycle, Policy, Project, ProjectId, UserId, Visibility,
};
use crate::permission::{Denied, Permission, Role, authorize, can_see_listing};
use crate::store::{Guard, Store, StoreError, Write};

pub const MAX_KEY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("there is no such project")]
    NotFound,
    #[error("{0}")]
    Denied(#[from] Denied),
    #[error("{0}")]
    Rule(#[from] RuleError),
    #[error("{0}")]
    Join(#[from] JoinError),
    #[error("{0}")]
    Invalid(#[from] Invalid),
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("the project cannot move from {from:?} to {to:?}")]
    Lifecycle { from: Lifecycle, to: Lifecycle },
    #[error("an idempotency key is 1 to {MAX_KEY} letters, digits or hyphens")]
    Key,
}

/// Who is asking and when. `user` comes from the authenticated session.
#[derive(Debug, Clone)]
pub struct Call {
    pub user: UserId,
    pub now: u64,
    pub correlation: String,
}

#[derive(Debug, Clone)]
pub struct NewProject {
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub visibility: Visibility,
    pub join_policy: JoinPolicy,
}

/// Makes ids that cannot be guessed. Passed in, like the clock.
pub type Ids = dyn Fn() -> String + Send + Sync;

pub struct Projects<'a, S: Store> {
    pub store: &'a S,
    pub ids: &'a Ids,
}

impl<S: Store> std::fmt::Debug for Projects<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Projects").finish_non_exhaustive()
    }
}

pub(crate) fn event(
    call: &Call,
    project: &Project,
    action: &str,
    resource: &str,
    details: &[(&str, &str)],
) -> audit::Event {
    audit::Event {
        at: call.now,
        actor: Actor::User(call.user.clone()),
        action: action.to_owned(),
        project: project.id.clone(),
        resource: resource.to_owned(),
        policy_version: project.policy_version,
        correlation: call.correlation.clone(),
        details: audit::scrub(details),
    }
}

/// The project and the actor as they were read.
pub(crate) fn guards(project: &Project, actor: &Membership) -> Vec<Guard> {
    vec![
        Guard::Project(project.id.clone(), project.revision),
        Guard::Member(project.id.clone(), actor.user.clone(), actor.revision),
    ]
}

/// A request left pending when its project or membership no longer wants an answer.
pub(crate) fn close(call: &Call, request: &JoinRequest) -> Result<Write, Error> {
    let over = if call.now < request.expires_at {
        join::State::Cancelled
    } else {
        join::State::Expired
    };
    let done = join::settle(request, request.revision, over, &call.user, call.now)?;
    Ok(Write::Join(done))
}

pub(crate) fn check_key(key: Option<&str>) -> Result<(), Error> {
    let ok = key.is_none_or(|k| {
        (1..=MAX_KEY).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    ok.then_some(()).ok_or(Error::Key)
}

impl<S: Store> Projects<'_, S> {
    /// The project and the actor's membership. A project the actor may not even see is reported as missing.
    pub(crate) fn seen(
        &self,
        call: &Call,
        id: &ProjectId,
    ) -> Result<(Project, Option<Membership>), Error> {
        let project = self.store.project(id)?.ok_or(Error::NotFound)?;
        let member = self.store.member(id, &call.user)?;
        if !can_see_listing(&project, member.as_ref()) {
            return Err(Error::NotFound);
        }
        Ok((project, member))
    }

    pub(crate) fn allowed(
        &self,
        call: &Call,
        id: &ProjectId,
        permission: Permission,
    ) -> Result<(Project, Membership), Error> {
        let (project, member) = self.seen(call, id)?;
        authorize(&project, member.as_ref(), permission)?;
        let member = member.ok_or(Error::Denied(Denied::NotAMember))?;
        Ok((project, member))
    }

    /// The new project is a draft with its creator as the only owner. Repeating a call with the same `key` returns
    /// the project it made the first time.
    pub fn create(
        &self,
        call: &Call,
        new: NewProject,
        key: Option<&str>,
    ) -> Result<Project, Error> {
        check_key(key)?;
        if let Some(key) = key
            && let Some(made) = self.store.key(&call.user, key)?
        {
            return self.view(call, &ProjectId(made));
        }
        model::check_listing(&new.slug, &new.name, &new.summary)?;
        let project = Project {
            format: FORMAT,
            id: ProjectId((self.ids)()),
            slug: new.slug,
            name: new.name,
            summary: new.summary,
            visibility: new.visibility,
            lifecycle: Lifecycle::Draft,
            join_policy: new.join_policy,
            policy: Policy::default(),
            policy_version: 1,
            revision: 1,
        };
        let owner = Membership::active(project.id.clone(), call.user.clone(), Role::Owner);
        let log = event(call, &project, "project.create", &project.id.0, &[]);
        let mut writes = vec![Write::Project(project.clone()), Write::Member(owner)];
        if let Some(key) = key {
            writes.push(Write::Key {
                user: call.user.clone(),
                key: key.to_owned(),
                resource: project.id.0.clone(),
            });
        }
        self.store.commit(Vec::new(), writes, vec![log])?;
        Ok(project)
    }

    pub fn view(&self, call: &Call, id: &ProjectId) -> Result<Project, Error> {
        self.seen(call, id).map(|(project, _)| project)
    }

    pub fn set_visibility(
        &self,
        call: &Call,
        id: &ProjectId,
        visibility: Visibility,
    ) -> Result<Project, Error> {
        // Taking a project out of view is allowed in more states than publishing it.
        let needed = if visibility == Visibility::Private {
            Permission::Unpublish
        } else {
            Permission::EditSettings
        };
        let (mut project, actor) = self.allowed(call, id, needed)?;
        let guards = guards(&project, &actor);
        project.visibility = visibility;
        project.policy_version += 1;
        project.revision += 1;
        let to = format!("{visibility:?}");
        let log = event(call, &project, "project.visibility", &id.0, &[("to", &to)]);
        self.store
            .commit(guards, vec![Write::Project(project.clone())], vec![log])?;
        Ok(project)
    }

    /// Activate, archive, restore or ask for deletion. Suspension is the platform's, not a member's.
    pub fn set_lifecycle(
        &self,
        call: &Call,
        id: &ProjectId,
        to: Lifecycle,
    ) -> Result<Project, Error> {
        let current = self.store.project(id)?.ok_or(Error::NotFound)?;
        // Asking for deletion, and taking that back, are the owner's.
        let needed = if to == Lifecycle::DeletionPending
            || current.lifecycle == Lifecycle::DeletionPending
        {
            Permission::DeleteProject
        } else {
            Permission::ChangeLifecycle
        };
        let (mut project, actor) = self.allowed(call, id, needed)?;
        let guards = guards(&project, &actor);
        let from = project.lifecycle;
        // Repeating a transition that already happened is not an error (§3.1).
        if from == to {
            return Ok(project);
        }
        let platform_only =
            matches!(to, Lifecycle::Suspended | Lifecycle::Deleted) || from == Lifecycle::Suspended;
        if platform_only || !from.can_become(to) {
            return Err(Error::Lifecycle { from, to });
        }
        project.lifecycle = to;
        project.revision += 1;
        let state = format!("{to:?}");
        let log = event(
            call,
            &project,
            "project.lifecycle",
            &id.0,
            &[("to", &state)],
        );
        let mut writes = vec![Write::Project(project.clone())];
        if to != Lifecycle::Active {
            // Room for the few that two callers can add past the cap at once.
            for request in self.store.pending_joins(id, call.now, MAX_PENDING * 2)? {
                writes.push(close(call, &request)?);
            }
        }
        self.store.commit(guards, writes, vec![log])?;
        Ok(project)
    }
}
