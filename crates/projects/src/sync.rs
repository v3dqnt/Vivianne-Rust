//! Repository access, tracked apart from membership (§5.3): an approved member may not have it yet.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[default]
    NotConfigured,
    NotRequired,
    Queued,
    InvitationCreated,
    InvitationPendingAcceptance,
    Active,
    BlockedByOrgPolicy,
    InsufficientAppPermissions,
    RepositoryNotAdministerable,
    ProviderRateLimited,
    ProviderUnavailable,
    RevocationQueued,
    RevocationBlocked,
    ManualActionRequired,
}

/// Why a provider refused or failed; raw provider payloads are never kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    OrgPolicy,
    AppPermissions,
    NotAdministerable,
    RateLimited,
    Unavailable,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Requested,
    NotNeeded,
    Invited,
    AwaitingAcceptance,
    Accepted,
    Failed(Failure),
    Retry,
    RevokeRequested,
    Revoked,
}

fn failed(failure: Failure) -> Access {
    match failure {
        Failure::OrgPolicy => Access::BlockedByOrgPolicy,
        Failure::AppPermissions => Access::InsufficientAppPermissions,
        Failure::NotAdministerable => Access::RepositoryNotAdministerable,
        Failure::RateLimited => Access::ProviderRateLimited,
        Failure::Unavailable => Access::ProviderUnavailable,
        Failure::Other => Access::ManualActionRequired,
    }
}

/// The next state, or `None` when the event does not apply (a late or repeated delivery is then ignored).
#[must_use]
pub fn next(from: Access, event: Event) -> Option<Access> {
    use Access as A;
    use Event as E;
    let revoking = matches!(from, A::RevocationQueued | A::RevocationBlocked);
    match event {
        // Once access is being taken away, nothing about granting it applies.
        E::Invited | E::AwaitingAcceptance | E::Accepted | E::Requested | E::NotNeeded
            if revoking =>
        {
            None
        }
        E::Requested => (from != A::Active).then_some(A::Queued),
        E::NotNeeded => Some(A::NotRequired),
        E::Invited => (from == A::Queued).then_some(A::InvitationCreated),
        E::AwaitingAcceptance => {
            (from == A::InvitationCreated).then_some(A::InvitationPendingAcceptance)
        }
        E::Accepted => matches!(
            from,
            A::Queued | A::InvitationCreated | A::InvitationPendingAcceptance
        )
        .then_some(A::Active),
        E::Failed(failure) if revoking => {
            let _ = failure;
            Some(A::RevocationBlocked)
        }
        E::Failed(failure) => Some(failed(failure)),
        E::Retry => match from {
            A::RevocationBlocked => Some(A::RevocationQueued),
            A::ProviderRateLimited | A::ProviderUnavailable | A::ManualActionRequired => {
                Some(A::Queued)
            }
            A::BlockedByOrgPolicy
            | A::InsufficientAppPermissions
            | A::RepositoryNotAdministerable => Some(A::Queued),
            _ => None,
        },
        E::RevokeRequested => (from != A::NotConfigured).then_some(A::RevocationQueued),
        E::Revoked => revoking.then_some(A::NotConfigured),
    }
}

pub const MAX_ATTEMPTS: u32 = 6;

/// How long to wait before trying again, or `None` when retrying by itself will not help (R3.3).
#[must_use]
pub fn retry_after_ms(failure: Failure, attempt: u32) -> Option<u64> {
    let transient = matches!(failure, Failure::RateLimited | Failure::Unavailable);
    (transient && attempt < MAX_ATTEMPTS).then(|| 30_000_u64 << attempt)
}

/// Take provider access away only if Vivianne granted it and no other project still needs it (§2.5).
#[must_use]
pub fn should_revoke(granted_by_us: bool, other_projects_needing: usize) -> bool {
    granted_by_us && other_projects_needing == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invitation_is_not_access_until_accepted() {
        let invited = next(Access::Queued, Event::Invited).expect("invited");
        let waiting = next(invited, Event::AwaitingAcceptance).expect("waiting");
        assert_ne!(waiting, Access::Active);
        assert_eq!(next(waiting, Event::Accepted), Some(Access::Active));
        assert_eq!(
            next(Access::Active, Event::Invited),
            None,
            "a repeated delivery"
        );
    }

    #[test]
    fn a_late_acceptance_cannot_undo_a_revocation() {
        let revoking =
            next(Access::InvitationPendingAcceptance, Event::RevokeRequested).expect("queued");
        assert_eq!(next(revoking, Event::Accepted), None);
        let blocked = next(revoking, Event::Failed(Failure::Unavailable)).expect("blocked");
        assert_eq!(blocked, Access::RevocationBlocked);
        assert_eq!(next(blocked, Event::Retry), Some(Access::RevocationQueued));
    }

    #[test]
    fn retries_are_capped_and_only_for_passing_trouble() {
        assert_eq!(retry_after_ms(Failure::RateLimited, 0), Some(30_000));
        assert_eq!(retry_after_ms(Failure::Unavailable, MAX_ATTEMPTS), None);
        assert_eq!(retry_after_ms(Failure::OrgPolicy, 0), None);
    }

    #[test]
    fn access_another_project_needs_is_kept() {
        assert!(should_revoke(true, 0));
        assert!(!should_revoke(true, 1));
        assert!(!should_revoke(false, 0));
    }
}
