//! The role table and the one authorization check (R1.6, R4.2). What is not granted here is denied.

use serde::{Deserialize, Serialize};

use crate::membership::{Membership, Status};
use crate::model::{Lifecycle, Policy, Project, Share, UserId, Visibility};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Contributor,
    Maintainer,
    Admin,
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ViewContent,
    EditSettings,
    /// Activate, archive and restore.
    ChangeLifecycle,
    /// Take the project out of public view.
    Unpublish,
    ReviewJoinRequests,
    InviteMembers,
    ChangeRoles,
    RemoveMembers,
    CreateTasks,
    RunAgents,
    SubmitChanges,
    ApproveChanges,
    ConnectRepository,
    ManagePolicy,
    TransferOwnership,
    DeleteProject,
}

impl Permission {
    pub const ALL: [Self; 16] = [
        Self::ViewContent,
        Self::EditSettings,
        Self::ChangeLifecycle,
        Self::Unpublish,
        Self::ReviewJoinRequests,
        Self::InviteMembers,
        Self::ChangeRoles,
        Self::RemoveMembers,
        Self::CreateTasks,
        Self::RunAgents,
        Self::SubmitChanges,
        Self::ApproveChanges,
        Self::ConnectRepository,
        Self::ManagePolicy,
        Self::TransferOwnership,
        Self::DeleteProject,
    ];
}

/// Whether `role` holds `permission` under `policy` in an active project (§4.1).
#[must_use]
pub fn grants(role: Role, permission: Permission, policy: &Policy) -> bool {
    use Permission as P;
    let maintainer_up = role >= Role::Maintainer;
    let admin_up = role >= Role::Admin;
    match permission {
        P::ViewContent => true,
        P::CreateTasks | P::SubmitChanges => role >= Role::Contributor,
        P::RunAgents => {
            maintainer_up || (role == Role::Contributor && policy.contributors_run_agents)
        }
        P::ApproveChanges => maintainer_up,
        P::ReviewJoinRequests => {
            admin_up || (role == Role::Maintainer && policy.maintainers_review_requests)
        }
        P::InviteMembers => admin_up || (role == Role::Maintainer && policy.maintainers_invite),
        P::EditSettings
        | P::ChangeLifecycle
        | P::Unpublish
        | P::ChangeRoles
        | P::RemoveMembers
        | P::ManagePolicy => admin_up,
        P::ConnectRepository | P::TransferOwnership | P::DeleteProject => role == Role::Owner,
    }
}

/// Whether the project's state leaves `permission` usable by `role`.
fn allowed_in(lifecycle: Lifecycle, role: Role, permission: Permission) -> bool {
    use Permission as P;
    match lifecycle {
        Lifecycle::Active => true,
        Lifecycle::Draft => role == Role::Owner,
        // Read-only, but membership can still be cut and the project handed on or deleted.
        Lifecycle::Archived => matches!(
            permission,
            P::ViewContent
                | P::ChangeLifecycle
                | P::Unpublish
                | P::RemoveMembers
                | P::TransferOwnership
                | P::DeleteProject
        ),
        // Lifting a suspension is the platform's, so no lifecycle change is listed.
        Lifecycle::Suspended => {
            matches!(permission, P::ViewContent | P::Unpublish | P::RemoveMembers)
        }
        Lifecycle::DeletionPending => {
            role == Role::Owner && matches!(permission, P::ViewContent | P::DeleteProject)
        }
        Lifecycle::Deleted => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    #[error("not a member of this project")]
    NotAMember,
    #[error("your membership is not active")]
    Inactive,
    #[error("your role does not allow this")]
    Role,
    #[error("the project's current state does not allow this")]
    Lifecycle,
}

/// The one check every operation makes. `membership` must be the stored record for the authenticated user in
/// this project; one for another project is refused.
pub fn authorize(
    project: &Project,
    membership: Option<&Membership>,
    permission: Permission,
) -> Result<(), Denied> {
    let member = membership
        .filter(|m| m.project == project.id)
        .ok_or(Denied::NotAMember)?;
    if member.status != Status::Active {
        return Err(Denied::Inactive);
    }
    if !grants(member.role, permission, &project.policy) {
        return Err(Denied::Role);
    }
    if !allowed_in(project.lifecycle, member.role, permission) {
        return Err(Denied::Lifecycle);
    }
    Ok(())
}

/// Whether the listing and public profile may be shown to this person (who may be nobody).
#[must_use]
pub fn can_see_listing(project: &Project, membership: Option<&Membership>) -> bool {
    let public = project.visibility == Visibility::Public
        && matches!(project.lifecycle, Lifecycle::Active | Lifecycle::Archived);
    // Someone invited may look before accepting.
    let invited = membership
        .is_some_and(|m| m.project == project.id && m.status == Status::Invited)
        && matches!(project.lifecycle, Lifecycle::Active | Lifecycle::Archived);
    public || invited || authorize(project, membership, Permission::ViewContent).is_ok()
}

/// Whether `reader` may read something `creator` made, shared as `share`. A public project makes nothing in it public.
#[must_use]
pub fn can_read(
    project: &Project,
    share: &Share,
    creator: &UserId,
    reader: &UserId,
    membership: Option<&Membership>,
) -> bool {
    if *share == Share::Public
        && project.visibility == Visibility::Public
        && project.policy.public_threads_allowed
        && project.lifecycle == Lifecycle::Active
    {
        return true;
    }
    let member = membership.filter(|m| m.user == *reader);
    if authorize(project, member, Permission::ViewContent).is_err() {
        return false;
    }
    reader == creator
        || match share {
            Share::Creator => false,
            Share::Members(users) => users.contains(reader),
            Share::AllMembers | Share::Public => true,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FORMAT, JoinPolicy};

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

    fn member(role: Role) -> Membership {
        Membership::active("p".into(), "u".into(), role)
    }

    #[test]
    fn the_table_matches_the_plan() {
        let p = Policy::default();
        let held = |role| {
            Permission::ALL
                .into_iter()
                .filter(|x| grants(role, *x, &p))
                .count()
        };
        assert_eq!(
            [
                held(Role::Viewer),
                held(Role::Contributor),
                held(Role::Maintainer),
                held(Role::Admin),
                held(Role::Owner)
            ],
            [1, 4, 5, 13, 16]
        );
        assert!(!grants(Role::Viewer, Permission::RunAgents, &p));
        assert!(!grants(Role::Contributor, Permission::ApproveChanges, &p));
        assert!(!grants(Role::Admin, Permission::TransferOwnership, &p));
    }

    #[test]
    fn no_membership_another_projects_or_an_inactive_one_is_denied() {
        let project = project();
        let see = Permission::ViewContent;
        assert_eq!(authorize(&project, None, see), Err(Denied::NotAMember));
        let elsewhere = Membership::active("q".into(), "u".into(), Role::Owner);
        assert_eq!(
            authorize(&project, Some(&elsewhere), see),
            Err(Denied::NotAMember)
        );
        let mut suspended = member(Role::Admin);
        suspended.status = Status::Suspended;
        assert_eq!(
            authorize(&project, Some(&suspended), see),
            Err(Denied::Inactive)
        );
    }

    #[test]
    fn an_archived_or_suspended_project_runs_no_agents() {
        for lifecycle in [
            Lifecycle::Archived,
            Lifecycle::Suspended,
            Lifecycle::Deleted,
        ] {
            let mut project = project();
            project.lifecycle = lifecycle;
            let owner = member(Role::Owner);
            assert!(authorize(&project, Some(&owner), Permission::RunAgents).is_err());
        }
    }

    #[test]
    fn a_public_project_does_not_publish_its_threads() {
        let mut project = project();
        let (creator, stranger) = (UserId::from("u"), UserId::from("x"));
        assert!(can_see_listing(&project, None));
        assert!(!can_read(
            &project,
            &Share::AllMembers,
            &creator,
            &stranger,
            None
        ));
        assert!(
            !can_read(&project, &Share::Public, &creator, &stranger, None),
            "policy off"
        );
        project.policy.public_threads_allowed = true;
        assert!(can_read(
            &project,
            &Share::Public,
            &creator,
            &stranger,
            None
        ));
        project.visibility = Visibility::Private;
        assert!(!can_read(
            &project,
            &Share::Public,
            &creator,
            &stranger,
            None
        ));
        assert!(!can_see_listing(&project, None));
    }

    #[test]
    fn a_private_thread_is_not_read_by_other_members_even_admins() {
        let project = project();
        let admin = Membership::active("p".into(), "a".into(), Role::Admin);
        let creator = UserId::from("u");
        let reader = UserId::from("a");
        assert!(!can_read(
            &project,
            &Share::Creator,
            &creator,
            &reader,
            Some(&admin)
        ));
        let shared = Share::Members(vec![reader.clone()]);
        assert!(can_read(&project, &shared, &creator, &reader, Some(&admin)));
        // Another member's record does not stand in for the reader's.
        assert!(!can_read(
            &project,
            &Share::AllMembers,
            &creator,
            &UserId::from("z"),
            Some(&admin)
        ));
    }
}
