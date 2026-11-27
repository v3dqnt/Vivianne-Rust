//! Join requests (§2.2, §5.1). A request grants nothing by itself.

use serde::{Deserialize, Serialize};

use crate::membership::{Membership, Status};
use crate::model::has_controls;
use crate::model::{JoinPolicy, Lifecycle, Project, ProjectId, UserId, Visibility};

pub const MAX_NOTE: usize = 500;
/// The most requests a project holds waiting.
pub const MAX_PENDING: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Approved,
    Rejected,
    Withdrawn,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub id: String,
    pub project: ProjectId,
    pub user: UserId,
    pub note: String,
    pub state: State,
    pub created_at: u64,
    pub expires_at: u64,
    pub decided_at: Option<u64>,
    pub decided_by: Option<UserId>,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    /// Also what a private project says, so asking does not reveal that it exists.
    #[error("there is no such project")]
    NotFound,
    #[error("this project is not taking requests")]
    Closed,
    #[error("you already have a membership here")]
    AlreadyMember,
    #[error("you cannot request to join this project")]
    Blocked,
    #[error("you already have a request waiting")]
    Pending,
    #[error("wait before asking again")]
    Cooldown,
    #[error("the project is full")]
    Full,
    #[error("the project has too many requests waiting")]
    Queue,
    #[error("the note is longer than {MAX_NOTE} characters or has control characters")]
    Note,
    #[error("the request was already decided or has expired")]
    Settled,
    #[error("the request changed since you read it")]
    Stale,
    #[error("only the person who asked can withdraw it")]
    NotYours,
}

/// What is known when someone asks to join.
#[derive(Debug, Clone, Copy)]
pub struct Asking<'a> {
    pub membership: Option<&'a Membership>,
    /// This person's earlier requests for this project.
    pub earlier: &'a [JoinRequest],
    pub blocked: bool,
    pub members: usize,
    /// Requests already waiting for this project.
    pub waiting: usize,
    pub now: u64,
}

pub fn may_request(project: &Project, asking: Asking<'_>) -> Result<(), JoinError> {
    if project.visibility != Visibility::Public || project.lifecycle == Lifecycle::Draft {
        return Err(JoinError::NotFound);
    }
    if asking.blocked {
        return Err(JoinError::Blocked);
    }
    if asking
        .membership
        .is_some_and(|m| m.status != Status::Removed)
    {
        return Err(JoinError::AlreadyMember);
    }
    let open = matches!(project.join_policy, JoinPolicy::Open | JoinPolicy::Approval);
    if project.lifecycle != Lifecycle::Active || !open {
        return Err(JoinError::Closed);
    }
    for earlier in asking.earlier {
        if earlier.state == State::Pending && asking.now < earlier.expires_at {
            return Err(JoinError::Pending);
        }
        let cooling = earlier
            .decided_at
            .is_some_and(|at| asking.now < at.saturating_add(project.policy.rejection_cooldown_ms));
        if earlier.state == State::Rejected && cooling {
            return Err(JoinError::Cooldown);
        }
    }
    if asking.members >= project.policy.max_members {
        return Err(JoinError::Full);
    }
    if asking.waiting >= MAX_PENDING {
        return Err(JoinError::Queue);
    }
    Ok(())
}

pub fn new_request(
    project: &Project,
    id: &str,
    user: UserId,
    note: &str,
    now: u64,
) -> Result<JoinRequest, JoinError> {
    if note.chars().count() > MAX_NOTE || has_controls(note) {
        return Err(JoinError::Note);
    }
    Ok(JoinRequest {
        id: id.to_owned(),
        project: project.id.clone(),
        user,
        note: note.to_owned(),
        state: State::Pending,
        created_at: now,
        expires_at: now.saturating_add(project.policy.request_lifetime_ms),
        decided_at: None,
        decided_by: None,
        revision: 1,
    })
}

/// The request moved out of `Pending`. `read` is the revision the decider saw, so two decisions cannot both land.
pub fn settle(
    request: &JoinRequest,
    read: u64,
    to: State,
    by: &UserId,
    now: u64,
) -> Result<JoinRequest, JoinError> {
    if request.revision != read {
        return Err(JoinError::Stale);
    }
    if request.state != State::Pending || to == State::Pending {
        return Err(JoinError::Settled);
    }
    if now >= request.expires_at && to != State::Expired {
        return Err(JoinError::Settled);
    }
    if to == State::Withdrawn && *by != request.user {
        return Err(JoinError::NotYours);
    }
    Ok(JoinRequest {
        state: to,
        decided_at: Some(now),
        decided_by: Some(by.clone()),
        revision: request.revision + 1,
        ..request.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FORMAT, Policy};
    use crate::permission::Role;

    fn project() -> Project {
        Project {
            format: FORMAT,
            id: "p".into(),
            slug: "abc".into(),
            name: "n".into(),
            summary: String::new(),
            visibility: Visibility::Public,
            lifecycle: Lifecycle::Active,
            join_policy: JoinPolicy::Approval,
            policy: Policy::default(),
            policy_version: 1,
            revision: 1,
        }
    }

    fn asking(earlier: &[JoinRequest]) -> Asking<'_> {
        Asking {
            membership: None,
            earlier,
            blocked: false,
            members: 1,
            waiting: 0,
            now: 1000,
        }
    }

    fn request() -> JoinRequest {
        new_request(&project(), "r", "u".into(), "hi", 1000).expect("made")
    }

    #[test]
    fn private_projects_look_missing_and_blocked_people_stay_out() {
        let mut private = project();
        private.visibility = Visibility::Private;
        assert_eq!(may_request(&private, asking(&[])), Err(JoinError::NotFound));
        let blocked = Asking {
            blocked: true,
            ..asking(&[])
        };
        assert_eq!(may_request(&project(), blocked), Err(JoinError::Blocked));
        let suspended = {
            let mut m = Membership::active("p".into(), "u".into(), Role::Viewer);
            m.status = Status::Suspended;
            m
        };
        let again = Asking {
            membership: Some(&suspended),
            ..asking(&[])
        };
        assert_eq!(
            may_request(&project(), again),
            Err(JoinError::AlreadyMember)
        );
    }

    #[test]
    fn one_pending_request_a_cooldown_after_rejection_and_a_capacity() {
        let pending = [request()];
        assert_eq!(
            may_request(&project(), asking(&pending)),
            Err(JoinError::Pending)
        );
        let rejected =
            [settle(&request(), 1, State::Rejected, &"a".into(), 1000).expect("rejected")];
        assert_eq!(
            may_request(&project(), asking(&rejected)),
            Err(JoinError::Cooldown)
        );
        let later = Asking {
            now: u64::MAX,
            ..asking(&rejected)
        };
        assert_eq!(may_request(&project(), later), Ok(()));
        let full = Asking {
            members: 100,
            ..asking(&[])
        };
        assert_eq!(may_request(&project(), full), Err(JoinError::Full));
    }

    #[test]
    fn of_two_racing_decisions_only_one_lands() {
        let request = request();
        let approved = settle(&request, 1, State::Approved, &"a".into(), 1001).expect("first");
        assert_eq!(
            settle(&approved, 1, State::Rejected, &"b".into(), 1002),
            Err(JoinError::Stale)
        );
        assert_eq!(
            settle(&approved, 2, State::Rejected, &"b".into(), 1002),
            Err(JoinError::Settled)
        );
    }

    #[test]
    fn only_the_asker_withdraws_and_an_expired_request_cannot_be_approved() {
        let request = request();
        let other = UserId::from("x");
        assert_eq!(
            settle(&request, 1, State::Withdrawn, &other, 1001),
            Err(JoinError::NotYours)
        );
        let late = request.expires_at;
        assert_eq!(
            settle(&request, 1, State::Approved, &other, late),
            Err(JoinError::Settled)
        );
        assert!(settle(&request, 1, State::Expired, &other, late).is_ok());
        let long = "x".repeat(MAX_NOTE + 1);
        assert_eq!(
            new_request(&project(), "r", other, &long, 0),
            Err(JoinError::Note)
        );
    }
}
