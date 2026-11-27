//! An agent run is a delegated actor (§4.4): its permissions are an intersection, so every input can only narrow
//! them, and `core::Mode` applies on top (R4.1, R4.3). Merging is not something an agent can be granted.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use vivianne_core::Mode;

use crate::membership::Membership;
use crate::model::{Project, ProjectId, UserId};
use crate::permission::{Denied, Permission, Role, authorize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadRepository,
    WriteWorkspace,
    RunCommands,
    Network,
    UseSecrets,
    Publish,
}

impl Capability {
    pub const ALL: [Self; 6] = [
        Self::ReadRepository,
        Self::WriteWorkspace,
        Self::RunCommands,
        Self::Network,
        Self::UseSecrets,
        Self::Publish,
    ];
}

pub type Capabilities = BTreeSet<Capability>;

/// The most a run started by someone with `role` could hold.
#[must_use]
pub fn for_role(role: Role) -> Capabilities {
    use Capability as C;
    let held: &[C] = match role {
        Role::Viewer => &[],
        Role::Contributor => &[
            C::ReadRepository,
            C::WriteWorkspace,
            C::RunCommands,
            C::Publish,
        ],
        Role::Maintainer => &[
            C::ReadRepository,
            C::WriteWorkspace,
            C::RunCommands,
            C::Publish,
            C::Network,
        ],
        Role::Admin | Role::Owner => &Capability::ALL,
    };
    held.iter().copied().collect()
}

/// The limits that apply to one run, beside its initiator's role.
#[derive(Debug, Clone, Copy)]
pub struct Limits<'a> {
    pub project_policy: &'a Capabilities,
    pub task_scope: &'a Capabilities,
    pub run_grant: &'a Capabilities,
    pub platform: &'a Capabilities,
}

/// `human ∩ policy ∩ task ∩ run ∩ platform`; in Plan mode, only reading is left.
#[must_use]
pub fn effective(role: Role, limits: Limits<'_>, mode: Mode) -> Capabilities {
    for_role(role)
        .into_iter()
        .filter(|c| {
            limits.project_policy.contains(c)
                && limits.task_scope.contains(c)
                && limits.run_grant.contains(c)
                && limits.platform.contains(c)
                && (mode != Mode::Plan || *c == Capability::ReadRepository)
        })
        .collect()
}

/// What is recorded about a run, pinned when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub initiated_by: UserId,
    pub project: ProjectId,
    pub task: String,
    pub profile: String,
    pub profile_version: u32,
    pub policy_version: u32,
    pub workspace: String,
    pub base_commit: String,
    pub capabilities: Capabilities,
    pub started_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Stop {
    #[error("{0}")]
    Denied(#[from] Denied),
    #[error("the project's policy changed since the run started")]
    PolicyChanged,
    #[error("the run was not granted this")]
    NotGranted,
}

/// Asked when a queued run starts and again before each side effect (§7.3): the initiator must still be allowed,
/// the policy must be the one the run was pinned to, and the run must hold the capability. A promotion since the
/// start adds nothing; a demotion takes effect at once.
pub fn reauthorize(
    run: &Run,
    project: &Project,
    initiator: Option<&Membership>,
    needs: Capability,
) -> Result<(), Stop> {
    let member = initiator.filter(|m| m.user == run.initiated_by);
    authorize(project, member, Permission::RunAgents)?;
    if run.project != project.id || run.policy_version != project.policy_version {
        return Err(Stop::PolicyChanged);
    }
    let still = member.is_some_and(|m| for_role(m.role).contains(&needs));
    if !still || !run.capabilities.contains(&needs) {
        return Err(Stop::NotGranted);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::Status;
    use crate::model::{FORMAT, JoinPolicy, Lifecycle, Policy, Visibility};

    fn all() -> Capabilities {
        Capability::ALL.into_iter().collect()
    }

    fn limits(set: &Capabilities) -> Limits<'_> {
        Limits {
            project_policy: set,
            task_scope: set,
            run_grant: set,
            platform: set,
        }
    }

    fn project() -> Project {
        Project {
            format: FORMAT,
            id: "p".into(),
            slug: "abc".into(),
            name: "n".into(),
            summary: String::new(),
            visibility: Visibility::Private,
            lifecycle: Lifecycle::Active,
            join_policy: JoinPolicy::Approval,
            policy: Policy::default(),
            policy_version: 3,
            revision: 1,
        }
    }

    fn run(capabilities: Capabilities) -> Run {
        Run {
            id: "r".into(),
            initiated_by: "u".into(),
            project: "p".into(),
            task: "t".into(),
            profile: "default".into(),
            profile_version: 1,
            policy_version: 3,
            workspace: "w".into(),
            base_commit: "abc123".into(),
            capabilities,
            started_at: 0,
        }
    }

    #[test]
    fn an_owner_starting_a_run_does_not_make_it_an_owner() {
        let all = all();
        let narrow: Capabilities = [Capability::ReadRepository].into_iter().collect();
        let got = effective(
            Role::Owner,
            Limits {
                task_scope: &narrow,
                ..limits(&all)
            },
            Mode::Auto,
        );
        assert_eq!(got, narrow);
        let plan = effective(Role::Owner, limits(&all), Mode::Plan);
        assert_eq!(plan, narrow);
        assert!(effective(Role::Viewer, limits(&all), Mode::Auto).is_empty());
        assert!(
            !effective(Role::Contributor, limits(&all), Mode::Auto)
                .contains(&Capability::UseSecrets)
        );
    }

    #[test]
    fn removal_suspension_policy_change_and_demotion_stop_a_run() {
        let member = Membership::active("p".into(), "u".into(), Role::Admin);
        let run = run(all());
        let publish = Capability::Publish;
        assert_eq!(
            reauthorize(&run, &project(), Some(&member), publish),
            Ok(())
        );
        assert!(reauthorize(&run, &project(), None, publish).is_err());
        let mut suspended = member.clone();
        suspended.status = Status::Suspended;
        assert!(reauthorize(&run, &project(), Some(&suspended), publish).is_err());
        let mut changed = project();
        changed.policy_version = 4;
        assert_eq!(
            reauthorize(&run, &changed, Some(&member), publish),
            Err(Stop::PolicyChanged)
        );
        let mut demoted = member.clone();
        demoted.role = Role::Contributor;
        let secrets = Capability::UseSecrets;
        assert_eq!(
            reauthorize(&run, &project(), Some(&demoted), secrets),
            Err(Stop::NotGranted)
        );
        // Someone else's membership does not authorize this run.
        let other = Membership::active("p".into(), "z".into(), Role::Owner);
        assert!(reauthorize(&run, &project(), Some(&other), publish).is_err());
    }

    #[test]
    fn a_promotion_never_widens_a_running_run() {
        let promoted = Membership::active("p".into(), "u".into(), Role::Owner);
        let run = run([Capability::ReadRepository].into_iter().collect());
        let got = reauthorize(&run, &project(), Some(&promoted), Capability::Network);
        assert_eq!(got, Err(Stop::NotGranted));
    }
}
