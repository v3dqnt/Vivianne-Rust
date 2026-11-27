//! The operations on people: join requests, invitations, roles, removal and ownership.

use crate::join::{self, Asking, JoinError, JoinRequest, MAX_PENDING};
use crate::membership::{self, Membership, RuleError, Status};
use crate::model::{JoinPolicy, Lifecycle, Project, ProjectId, UserId};
use crate::permission::{Denied, Permission, Role};
use crate::service::{Call, Error, Projects, check_key, close, event, guards};
use crate::store::{Guard, Store, Write};

/// How long an ownership offer waits to be accepted.
const TRANSFER_LIFETIME_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// The membership an approved person ends up with. An approval only brings in someone who is out: it never lifts a
/// suspension or overrides an invitation, which need their own permissions.
fn joined(
    existing: Option<&Membership>,
    project: &Project,
    user: &UserId,
    role: Role,
) -> Result<Membership, Error> {
    Ok(match existing {
        None => Membership::active(project.id.clone(), user.clone(), role),
        Some(old) if old.status == Status::Removed => Membership {
            role,
            ..membership::moved(old, Status::Active)?
        },
        Some(_) => return Err(JoinError::AlreadyMember.into()),
    })
}

impl<S: Store> Projects<'_, S> {
    fn target(&self, project: &Project, user: &UserId) -> Result<Membership, Error> {
        self.store
            .member(&project.id, user)?
            .ok_or(Error::Denied(Denied::NotAMember))
    }

    /// A membership settles the question a pending request asked, so such requests are closed with it.
    fn closing_requests(
        &self,
        call: &Call,
        id: &ProjectId,
        user: &UserId,
    ) -> Result<Vec<Write>, Error> {
        self.store
            .joins_of(id, user)?
            .iter()
            .filter(|r| r.state == join::State::Pending)
            .map(|r| close(call, r))
            .collect()
    }

    /// Ask to join. With an open join policy the membership is made at once. Repeating a call with the same `key`
    /// returns the request it made the first time.
    pub fn request_join(
        &self,
        call: &Call,
        id: &ProjectId,
        note: &str,
        key: Option<&str>,
    ) -> Result<JoinRequest, Error> {
        check_key(key)?;
        if let Some(key) = key
            && let Some(made) = self.store.key(&call.user, key)?
        {
            return self
                .store
                .join(&made)?
                .filter(|r| r.user == call.user && r.project == *id)
                .ok_or(Error::NotFound);
        }
        let project = self.store.project(id)?.ok_or(Error::NotFound)?;
        let membership = self.store.member(id, &call.user)?;
        let earlier = self.store.joins_of(id, &call.user)?;
        let members = self.store.member_count(id)?;
        join::may_request(
            &project,
            Asking {
                membership: membership.as_ref(),
                earlier: &earlier,
                blocked: self.store.is_blocked(id, &call.user)?,
                members,
                waiting: self.store.pending_joins(id, call.now, MAX_PENDING)?.len(),
                now: call.now,
            },
        )?;
        let request_id = (self.ids)();
        let mut request =
            join::new_request(&project, &request_id, call.user.clone(), note, call.now)?;
        let log = event(call, &project, "join.request", &request_id, &[]);
        let mut writes = Vec::new();
        let mut guards = vec![Guard::Project(id.clone(), project.revision)];
        if project.join_policy == JoinPolicy::Open {
            guards.push(Guard::Members(id.clone(), members));
            request.state = join::State::Approved;
            request.decided_at = Some(call.now);
            writes.push(Write::Member(joined(
                membership.as_ref(),
                &project,
                &call.user,
                Role::Contributor,
            )?));
        }
        writes.push(Write::Join(request.clone()));
        if let Some(key) = key {
            writes.push(Write::Key {
                user: call.user.clone(),
                key: key.to_owned(),
                resource: request_id,
            });
        }
        self.store.commit(guards, writes, vec![log])?;
        Ok(request)
    }

    pub fn withdraw_join(&self, call: &Call, request_id: &str) -> Result<JoinRequest, Error> {
        let request = self
            .store
            .join(request_id)?
            .filter(|r| r.user == call.user)
            .ok_or(Error::NotFound)?;
        let project = self
            .store
            .project(&request.project)?
            .ok_or(Error::NotFound)?;
        let done = join::settle(
            &request,
            request.revision,
            join::State::Withdrawn,
            &call.user,
            call.now,
        )?;
        let log = event(call, &project, "join.withdraw", request_id, &[]);
        self.store
            .commit(Vec::new(), vec![Write::Join(done.clone())], vec![log])?;
        Ok(done)
    }

    /// Approve (with the role to give) or reject. `read` is the revision of the request the reviewer saw.
    pub fn decide_join(
        &self,
        call: &Call,
        request_id: &str,
        read: u64,
        approve_as: Option<Role>,
    ) -> Result<JoinRequest, Error> {
        let request = self.store.join(request_id)?.ok_or(Error::NotFound)?;
        let (project, reviewer) =
            self.allowed(call, &request.project, Permission::ReviewJoinRequests)?;
        let state = if approve_as.is_some() {
            join::State::Approved
        } else {
            join::State::Rejected
        };
        let done = join::settle(&request, read, state, &call.user, call.now)?;
        let mut writes = vec![Write::Join(done.clone())];
        let mut guards = guards(&project, &reviewer);
        if let Some(role) = approve_as {
            if !membership::can_assign(reviewer.role, role) {
                return Err(RuleError::Rank.into());
            }
            if self.store.is_blocked(&project.id, &request.user)? {
                return Err(JoinError::Blocked.into());
            }
            let members = self.store.member_count(&project.id)?;
            if members >= project.policy.max_members {
                return Err(JoinError::Full.into());
            }
            guards.push(Guard::Members(project.id.clone(), members));
            let existing = self.store.member(&project.id, &request.user)?;
            writes.push(Write::Member(joined(
                existing.as_ref(),
                &project,
                &request.user,
                role,
            )?));
        }
        let verdict = format!("{state:?}");
        let log = event(
            call,
            &project,
            "join.decide",
            request_id,
            &[("as", &verdict)],
        );
        self.store.commit(guards, writes, vec![log])?;
        Ok(done)
    }

    pub fn invite(
        &self,
        call: &Call,
        id: &ProjectId,
        user: &UserId,
        role: Role,
    ) -> Result<Membership, Error> {
        let (project, inviter) = self.allowed(call, id, Permission::InviteMembers)?;
        if !membership::can_assign(inviter.role, role) {
            return Err(RuleError::Rank.into());
        }
        if self.store.is_blocked(id, user)? {
            return Err(JoinError::Blocked.into());
        }
        let members = self.store.member_count(id)?;
        if members >= project.policy.max_members {
            return Err(JoinError::Full.into());
        }
        let record = match self.store.member(id, user)? {
            None => Membership {
                status: Status::Invited,
                ..Membership::active(id.clone(), user.clone(), role)
            },
            Some(old) => Membership {
                role,
                ..membership::moved(&old, Status::Invited)?
            },
        };
        let log = event(call, &project, "member.invite", &user.0, &[]);
        let mut guards = guards(&project, &inviter);
        guards.push(Guard::Members(id.clone(), members));
        let mut writes = self.closing_requests(call, id, user)?;
        writes.push(Write::Member(record.clone()));
        self.store.commit(guards, writes, vec![log])?;
        Ok(record)
    }

    pub fn accept_invite(&self, call: &Call, id: &ProjectId) -> Result<Membership, Error> {
        let project = self.store.project(id)?.ok_or(Error::NotFound)?;
        let invited = self
            .store
            .member(id, &call.user)?
            .filter(|m| m.status == Status::Invited)
            .ok_or(Error::NotFound)?;
        if project.lifecycle != Lifecycle::Active {
            return Err(Denied::Lifecycle.into());
        }
        let member = membership::moved(&invited, Status::Active)?;
        let log = event(call, &project, "member.accept", &call.user.0, &[]);
        let mut writes = self.closing_requests(call, id, &call.user)?;
        writes.push(Write::Member(member.clone()));
        let guards = vec![Guard::Project(id.clone(), project.revision)];
        self.store.commit(guards, writes, vec![log])?;
        Ok(member)
    }

    pub fn change_role(
        &self,
        call: &Call,
        id: &ProjectId,
        user: &UserId,
        role: Role,
    ) -> Result<Membership, Error> {
        let (project, actor) = self.allowed(call, id, Permission::ChangeRoles)?;
        let target = self.target(&project, user)?;
        membership::check_role_change(&actor, &target, role)?;
        let changed = Membership {
            role,
            revision: target.revision + 1,
            ..target
        };
        let to = format!("{role:?}");
        let log = event(call, &project, "member.role", &user.0, &[("to", &to)]);
        self.store.commit(
            guards(&project, &actor),
            vec![Write::Member(changed.clone())],
            vec![log],
        )?;
        Ok(changed)
    }

    /// Suspend, reinstate or remove. Removing yourself is leaving and needs no permission. `block` also keeps the
    /// person from asking again.
    pub fn set_member_status(
        &self,
        call: &Call,
        id: &ProjectId,
        user: &UserId,
        to: Status,
        block: bool,
    ) -> Result<Membership, Error> {
        let leaving = *user == call.user && to == Status::Removed;
        let (project, actor) = if leaving {
            // No membership reads the same as no project, so leaving cannot be used to probe.
            let me = self.store.member(id, user)?.ok_or(Error::NotFound)?;
            (self.store.project(id)?.ok_or(Error::NotFound)?, me)
        } else {
            self.allowed(call, id, Permission::RemoveMembers)?
        };
        let target = self.target(&project, user)?;
        membership::check_removal(&actor, &target)?;
        // Leaving would turn a suspension into a removal, from which one may ask to join again.
        if leaving && target.status == Status::Suspended {
            return Err(Denied::Inactive.into());
        }
        // Becoming active again only undoes a suspension; it is not a way around an invitation or a removal.
        let invalid =
            to == Status::Invited || (to == Status::Active && target.status != Status::Suspended);
        if invalid {
            return Err(RuleError::Transition {
                from: target.status,
                to,
            }
            .into());
        }
        let changed = membership::moved(&target, to)?;
        let state = format!("{to:?}");
        let log = event(call, &project, "member.status", &user.0, &[("to", &state)]);
        let mut writes = vec![Write::Member(changed.clone())];
        if block && !leaving {
            writes.push(Write::Block(id.clone(), user.clone()));
        }
        self.store
            .commit(guards(&project, &actor), writes, vec![log])?;
        Ok(changed)
    }

    pub fn offer_ownership(
        &self,
        call: &Call,
        id: &ProjectId,
        to: &UserId,
        confirmed: bool,
    ) -> Result<(), Error> {
        let (project, owner) = self.allowed(call, id, Permission::TransferOwnership)?;
        let next = self.target(&project, to)?;
        let expires_at = call.now.saturating_add(TRANSFER_LIFETIME_MS);
        let transfer = membership::offer(&owner, &next, confirmed, expires_at)?;
        let log = event(call, &project, "owner.offer", &to.0, &[]);
        self.store.commit(
            guards(&project, &owner),
            vec![Write::Transfer(id.clone(), Some(transfer))],
            vec![log],
        )?;
        Ok(())
    }

    pub fn accept_ownership(&self, call: &Call, id: &ProjectId) -> Result<(), Error> {
        let (project, me) = self.allowed(call, id, Permission::ViewContent)?;
        let transfer = self.store.transfer(id)?.ok_or(Error::NotFound)?;
        let owner = self.target(&project, &transfer.from)?;
        let (old, new) = membership::accept(&transfer, &owner, &me, call.now)?;
        let log = event(call, &project, "owner.accept", &call.user.0, &[]);
        self.store.commit(
            guards(&project, &me),
            vec![
                Write::Member(old),
                Write::Member(new),
                Write::Transfer(id.clone(), None),
            ],
            vec![log],
        )?;
        Ok(())
    }
}
