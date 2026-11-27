//! Membership states and the rules about who may change whom (§4.3, §5.2).

use serde::{Deserialize, Serialize};

use crate::model::{ProjectId, UserId};
use crate::permission::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Invited,
    Active,
    Suspended,
    Removed,
}

impl Status {
    #[must_use]
    pub fn can_become(self, to: Self) -> bool {
        use Status::{Active, Invited, Removed, Suspended};
        matches!(
            (self, to),
            (Invited | Suspended, Active | Removed)
                | (Active, Suspended | Removed)
                | (Removed, Invited | Active)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub project: ProjectId,
    pub user: UserId,
    pub role: Role,
    pub status: Status,
    pub revision: u64,
}

impl Membership {
    #[must_use]
    pub fn active(project: ProjectId, user: UserId, role: Role) -> Self {
        Self {
            project,
            user,
            role,
            status: Status::Active,
            revision: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("you cannot change your own role")]
    OwnRole,
    #[error("ownership changes only by transfer")]
    Ownership,
    #[error("you cannot give or take a role at or above your own")]
    Rank,
    #[error("the owner must transfer ownership before leaving")]
    FinalOwner,
    #[error("a membership cannot move from {from:?} to {to:?}")]
    Transition { from: Status, to: Status },
    #[error("ownership can be offered only to an active member, by the owner")]
    Offer,
    #[error("only the person the offer names can accept it, before it expires")]
    Accept,
    #[error("confirm the transfer first")]
    Unconfirmed,
}

/// Whether someone with `actor` may hand out `role` (to a new or existing member).
#[must_use]
pub fn can_assign(actor: Role, role: Role) -> bool {
    match actor {
        Role::Owner => role != Role::Owner,
        Role::Admin => role < Role::Admin,
        Role::Maintainer => role < Role::Maintainer,
        Role::Contributor | Role::Viewer => false,
    }
}

/// Whether `actor` may give `target` the role `to`.
pub fn check_role_change(
    actor: &Membership,
    target: &Membership,
    to: Role,
) -> Result<(), RuleError> {
    if actor.user == target.user {
        return Err(RuleError::OwnRole);
    }
    if to == Role::Owner || target.role == Role::Owner {
        return Err(RuleError::Ownership);
    }
    if !can_assign(actor.role, target.role) || !can_assign(actor.role, to) {
        return Err(RuleError::Rank);
    }
    Ok(())
}

/// Whether `actor` may suspend or remove `target`. Leaving is removing yourself.
pub fn check_removal(actor: &Membership, target: &Membership) -> Result<(), RuleError> {
    if target.role == Role::Owner {
        return Err(RuleError::FinalOwner);
    }
    if actor.user == target.user {
        return Ok(());
    }
    // An admin may remove another admin, but nobody below admin removes anyone (§4.1).
    if actor.role >= Role::Admin {
        Ok(())
    } else {
        Err(RuleError::Rank)
    }
}

/// The record after moving to `to`, one revision on.
pub fn moved(member: &Membership, to: Status) -> Result<Membership, RuleError> {
    if !member.status.can_become(to) {
        return Err(RuleError::Transition {
            from: member.status,
            to,
        });
    }
    Ok(Membership {
        status: to,
        revision: member.revision + 1,
        ..member.clone()
    })
}

/// An ownership transfer waiting for the new owner to accept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub from: UserId,
    pub to: UserId,
    pub expires_at: u64,
}

/// The owner offers ownership. `confirmed` is the strong confirmation step (§4.3).
pub fn offer(
    owner: &Membership,
    to: &Membership,
    confirmed: bool,
    expires_at: u64,
) -> Result<Transfer, RuleError> {
    let valid = owner.role == Role::Owner
        && owner.status == Status::Active
        && to.status == Status::Active
        && owner.project == to.project
        && owner.user != to.user;
    if !valid {
        return Err(RuleError::Offer);
    }
    if !confirmed {
        return Err(RuleError::Unconfirmed);
    }
    Ok(Transfer {
        from: owner.user.clone(),
        to: to.user.clone(),
        expires_at,
    })
}

/// The new owner accepts: returns the old owner (now an admin) and the new one, to be saved together.
pub fn accept(
    transfer: &Transfer,
    owner: &Membership,
    to: &Membership,
    now: u64,
) -> Result<(Membership, Membership), RuleError> {
    let still_stands = transfer.from == owner.user
        && transfer.to == to.user
        && owner.role == Role::Owner
        && to.status == Status::Active
        && owner.project == to.project
        && now < transfer.expires_at;
    if !still_stands {
        return Err(RuleError::Accept);
    }
    let with = |m: &Membership, role| Membership {
        role,
        revision: m.revision + 1,
        ..m.clone()
    };
    Ok((with(owner, Role::Admin), with(to, Role::Owner)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(user: &str, role: Role) -> Membership {
        Membership::active("p".into(), user.into(), role)
    }

    #[test]
    fn nobody_escalates_themselves_or_hands_out_their_own_rank() {
        let admin = m("a", Role::Admin);
        assert_eq!(
            check_role_change(&admin, &admin, Role::Owner),
            Err(RuleError::OwnRole)
        );
        let other = m("b", Role::Contributor);
        assert_eq!(
            check_role_change(&admin, &other, Role::Admin),
            Err(RuleError::Rank)
        );
        assert_eq!(
            check_role_change(&admin, &other, Role::Owner),
            Err(RuleError::Ownership)
        );
        assert_eq!(
            check_role_change(&admin, &m("c", Role::Admin), Role::Viewer),
            Err(RuleError::Rank)
        );
        assert_eq!(check_role_change(&admin, &other, Role::Maintainer), Ok(()));
        let maintainer = m("d", Role::Maintainer);
        assert_eq!(
            check_role_change(&other, &maintainer, Role::Viewer),
            Err(RuleError::Rank)
        );
    }

    #[test]
    fn the_owner_cannot_be_removed_even_by_themselves() {
        let owner = m("o", Role::Owner);
        assert_eq!(check_removal(&owner, &owner), Err(RuleError::FinalOwner));
        assert_eq!(
            check_removal(&m("a", Role::Admin), &owner),
            Err(RuleError::FinalOwner)
        );
        let viewer = m("v", Role::Viewer);
        assert_eq!(
            check_removal(&viewer, &viewer),
            Ok(()),
            "anyone else may leave"
        );
        assert_eq!(
            check_removal(&viewer, &m("w", Role::Viewer)),
            Err(RuleError::Rank)
        );
    }

    #[test]
    fn a_transfer_needs_confirmation_the_named_person_and_time() {
        let (owner, next) = (m("o", Role::Owner), m("n", Role::Admin));
        assert_eq!(offer(&owner, &next, false, 10), Err(RuleError::Unconfirmed));
        assert_eq!(offer(&next, &owner, true, 10), Err(RuleError::Offer));
        let transfer = offer(&owner, &next, true, 10).expect("offered");
        assert_eq!(
            accept(&transfer, &owner, &m("x", Role::Admin), 5),
            Err(RuleError::Accept)
        );
        assert_eq!(
            accept(&transfer, &owner, &next, 10),
            Err(RuleError::Accept),
            "expired"
        );
        let (old, new) = accept(&transfer, &owner, &next, 9).expect("accepted");
        assert_eq!((old.role, new.role), (Role::Admin, Role::Owner));
    }

    #[test]
    fn a_removed_member_cannot_be_suspended_back_into_existence() {
        let mut gone = m("g", Role::Viewer);
        gone.status = Status::Removed;
        assert!(moved(&gone, Status::Suspended).is_err());
        assert_eq!(moved(&gone, Status::Invited).map(|m| m.revision), Ok(2));
    }
}
