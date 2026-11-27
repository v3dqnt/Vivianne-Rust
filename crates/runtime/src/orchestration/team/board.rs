//! The board and its rules: revisions, the eight actions, dependencies, write scopes and the limits. No files, no
//! agents.
//!
//! Port of `task-board.ts`, `task-graph.ts`, `task-view.ts` and `validation.ts`, with their error codes.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Most tasks on a board that are not deleted.
pub const MAX_TASKS: usize = 256;
const MAX_SUBJECT: usize = 200;
const MAX_DESCRIPTION: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    InProgress,
    Completed,
    /// Kept, so its number is never given to another task.
    Deleted,
}

/// Who holds a task: a stable id, and the name it is shown by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub revision: u64,
    pub subject: String,
    pub description: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub write_scopes: Vec<String>,
}

/// A task as the model reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub revision: u64,
    pub subject: String,
    pub description: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub blocked_by: Vec<String>,
    pub write_scopes: Vec<String>,
    /// Pending, with everything it depends on completed.
    pub ready: bool,
    pub write_scope_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct TeamError {
    pub code: &'static str,
    pub message: String,
}

pub(super) fn error(code: &'static str, message: impl Into<String>) -> TeamError {
    TeamError {
        code,
        message: message.into(),
    }
}

fn invalid(message: impl Into<String>) -> TeamError {
    error("TEAM_INVALID_ARGUMENT", message)
}

fn text(value: &str, field: &str, max: usize) -> Result<String, TeamError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid(format!("{field} must be non-empty")));
    }
    if value.chars().count() > max {
        return Err(invalid(format!("{field} exceeds {max} characters")));
    }
    Ok(value.to_owned())
}

/// A path inside the project, written one way.
fn write_scope(value: &str) -> Result<String, TeamError> {
    let normalized = value.replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    let normalized = normalized.trim_end_matches('/');
    let drive = normalized.as_bytes().get(1) == Some(&b':');
    let outside = normalized.is_empty()
        || normalized.starts_with('/')
        || drive
        || normalized.split('/').any(|s| matches!(s, "" | "." | ".."));
    if outside {
        return Err(error(
            "TEAM_INVALID_WRITE_SCOPE",
            format!("invalid workspace-relative write scope \"{value}\""),
        ));
    }
    Ok(normalized.to_owned())
}

fn write_scopes(values: &[String]) -> Result<Vec<String>, TeamError> {
    let mut scopes = Vec::new();
    for value in values {
        let scope = write_scope(value)?;
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    Ok(scopes)
}

fn overlaps(a: &str, b: &str) -> bool {
    let under = |a: &str, b: &str| a.strip_prefix(b).is_some_and(|rest| rest.starts_with('/'));
    a == b || under(a, b) || under(b, a)
}

#[derive(Debug, Clone, Default)]
pub struct Create {
    pub subject: String,
    pub description: String,
    pub blocked_by: Vec<String>,
    pub write_scopes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Claim,
    Release,
    Edit,
    SetDependencies,
    Complete,
    Reopen,
    Reassign,
    Delete,
}

impl Action {
    pub const NAMES: [&'static str; 8] = [
        "claim",
        "release",
        "edit",
        "set_dependencies",
        "complete",
        "reopen",
        "reassign",
        "delete",
    ];

    pub fn parse(name: &str) -> Result<Self, TeamError> {
        Ok(match name {
            "claim" => Self::Claim,
            "release" => Self::Release,
            "edit" => Self::Edit,
            "set_dependencies" => Self::SetDependencies,
            "complete" => Self::Complete,
            "reopen" => Self::Reopen,
            "reassign" => Self::Reassign,
            "delete" => Self::Delete,
            other => return Err(invalid(format!("unsupported task action {other}"))),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Update {
    pub task_id: String,
    pub expected_revision: u64,
    pub action: Action,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub blocked_by: Option<Vec<String>>,
    pub write_scopes: Option<Vec<String>>,
    /// For a reassignment: who to, already checked to be a member. `None` unassigns.
    pub owner: Option<Owner>,
}

impl Update {
    #[must_use]
    pub fn new(task_id: impl Into<String>, expected_revision: u64, action: Action) -> Self {
        Self {
            task_id: task_id.into(),
            expected_revision,
            action,
            subject: None,
            description: None,
            blocked_by: None,
            write_scopes: None,
            owner: None,
        }
    }
}

/// Who is changing the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub owner: Owner,
    pub lead: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    /// The number of the next task. It only goes up.
    pub next: u64,
    pub tasks: Vec<Task>,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            next: 1,
            tasks: Vec::new(),
        }
    }
}

impl Board {
    fn find(&self, id: &str) -> Result<&Task, TeamError> {
        self.tasks.iter().find(|t| t.id == id).ok_or_else(|| {
            error(
                "TEAM_TASK_NOT_FOUND",
                format!("team task \"{id}\" not found"),
            )
        })
    }

    fn ready(&self, task: &Task) -> bool {
        task.blocked_by.iter().all(|id| {
            self.find(id)
                .is_ok_and(|blocker| blocker.status == Status::Completed)
        })
    }

    fn view_of(&self, task: &Task) -> View {
        let warnings = self
            .tasks
            .iter()
            .filter(|other| other.id != task.id && other.status == Status::InProgress)
            .filter(|other| {
                task.write_scopes
                    .iter()
                    .any(|a| other.write_scopes.iter().any(|b| overlaps(a, b)))
            })
            .map(|other| format!("write scopes overlap with {}", other.id))
            .collect();
        View {
            id: task.id.clone(),
            revision: task.revision,
            subject: task.subject.clone(),
            description: task.description.clone(),
            status: task.status,
            owner: task.owner.as_ref().map(|o| o.name.clone()),
            blocked_by: task.blocked_by.clone(),
            write_scopes: task.write_scopes.clone(),
            ready: task.status == Status::Pending && self.ready(task),
            write_scope_warnings: warnings,
        }
    }

    pub fn get(&self, id: &str) -> Result<View, TeamError> {
        Ok(self.view_of(self.find(id)?))
    }

    /// Every task that is not deleted, oldest first, with the id of its owner.
    #[must_use]
    pub fn list(&self) -> Vec<(View, Option<String>)> {
        self.tasks
            .iter()
            .filter(|t| t.status != Status::Deleted)
            .map(|t| (self.view_of(t), t.owner.as_ref().map(|o| o.id.clone())))
            .collect()
    }

    fn dependencies(&self, ids: &[String], own: Option<&str>) -> Result<Vec<String>, TeamError> {
        let mut seen = Vec::new();
        for id in ids {
            if Some(id.as_str()) == own {
                return Err(error(
                    "TEAM_TASK_DEPENDENCY_CYCLE",
                    "a team task cannot block itself",
                ));
            }
            if seen.contains(id) {
                return Err(invalid(format!("duplicate blocker \"{id}\"")));
            }
            if self.find(id)?.status == Status::Deleted {
                return Err(error(
                    "TEAM_TASK_NOT_FOUND",
                    format!("blocker task \"{id}\" not found"),
                ));
            }
            seen.push(id.clone());
        }
        Ok(seen)
    }

    /// Whether `candidate`'s blockers, followed, lead back to it.
    fn cycles(&self, candidate: &Task) -> bool {
        let mut left: Vec<&str> = candidate.blocked_by.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        while let Some(id) = left.pop() {
            if id == candidate.id {
                return true;
            }
            if seen.insert(id)
                && let Ok(task) = self.find(id)
            {
                left.extend(task.blocked_by.iter().map(String::as_str));
            }
        }
        false
    }

    pub fn create(&mut self, request: &Create) -> Result<View, TeamError> {
        let live = self
            .tasks
            .iter()
            .filter(|t| t.status != Status::Deleted)
            .count();
        if live >= MAX_TASKS {
            return Err(error(
                "TEAM_TASK_LIMIT",
                format!("Team task limit {MAX_TASKS} reached"),
            ));
        }
        let task = Task {
            id: format!("task-{}", self.next),
            revision: 1,
            subject: text(&request.subject, "subject", MAX_SUBJECT)?,
            description: text(&request.description, "description", MAX_DESCRIPTION)?,
            status: Status::Pending,
            owner: None,
            blocked_by: self.dependencies(&request.blocked_by, None)?,
            write_scopes: write_scopes(&request.write_scopes)?,
        };
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| error("TEAM_TASK_LIMIT", "Team task id space exhausted"))?;
        self.tasks.push(task.clone());
        Ok(self.view_of(&task))
    }

    /// The lead's handing of a task to a member, or taking it from whoever holds it.
    fn reassign(&self, next: &mut Task, actor: &Actor, change: &Update) -> Result<(), TeamError> {
        if !actor.lead {
            return Err(error(
                "TEAM_LEAD_REQUIRED",
                "only the Team Lead can reassign tasks",
            ));
        }
        if !matches!(next.status, Status::Pending | Status::InProgress) {
            return Err(error(
                "TEAM_TASK_INVALID_TRANSITION",
                "only a pending or in-progress task can be reassigned",
            ));
        }
        if change.owner.is_none() {
            next.status = Status::Pending;
        } else if self.ready(next) {
            next.status = Status::InProgress;
        } else {
            return Err(error(
                "TEAM_TASK_BLOCKED",
                format!("team task \"{}\" is blocked", next.id),
            ));
        }
        next.owner.clone_from(&change.owner);
        Ok(())
    }

    /// Marks a task deleted, unless another still depends on it.
    fn delete(&self, next: &mut Task) -> Result<(), TeamError> {
        if let Some(dependent) = self
            .tasks
            .iter()
            .find(|t| t.status != Status::Deleted && t.blocked_by.contains(&next.id))
        {
            return Err(error(
                "TEAM_TASK_HAS_DEPENDENTS",
                format!(
                    "team task \"{}\" still blocks \"{}\"",
                    next.id, dependent.id
                ),
            ));
        }
        next.status = Status::Deleted;
        Ok(())
    }

    pub fn update(&mut self, actor: &Actor, change: &Update) -> Result<View, TeamError> {
        let current = self.find(&change.task_id)?;
        let id = current.id.clone();
        if current.revision != change.expected_revision {
            return Err(error(
                "TEAM_TASK_STALE_REVISION",
                format!(
                    "stale team task \"{id}\" revision {}; current revision is {}",
                    change.expected_revision, current.revision
                ),
            ));
        }
        if current.status == Status::Deleted {
            return Err(error(
                "TEAM_TASK_DELETED",
                format!("team task \"{id}\" is deleted"),
            ));
        }
        let owns = current
            .owner
            .as_ref()
            .is_some_and(|o| o.id == actor.owner.id);
        let may = || {
            if actor.lead || owns {
                Ok(())
            } else {
                Err(error(
                    "TEAM_TASK_UNAUTHORIZED",
                    "task mutation requires its owner or Team Lead",
                ))
            }
        };
        let transition = |message: &str| error("TEAM_TASK_INVALID_TRANSITION", message);
        let mut next = current.clone();
        match change.action {
            Action::Claim => {
                if next.owner.is_some() && !owns {
                    return Err(error(
                        "TEAM_TASK_ALREADY_CLAIMED",
                        format!("team task \"{id}\" is owned by another member"),
                    ));
                }
                if next.status != Status::Pending || !self.ready(&next) {
                    return Err(error(
                        "TEAM_TASK_BLOCKED",
                        format!("team task \"{id}\" is not ready to claim"),
                    ));
                }
                next.status = Status::InProgress;
                next.owner = Some(actor.owner.clone());
            }
            Action::Release => {
                may()?;
                if next.status != Status::InProgress {
                    return Err(transition("only an in-progress task can be released"));
                }
                next.status = Status::Pending;
                next.owner = None;
            }
            Action::Edit => {
                may()?;
                edit(&mut next, change)?;
            }
            Action::SetDependencies => {
                may()?;
                let blockers = change
                    .blocked_by
                    .as_ref()
                    .ok_or_else(|| invalid("set_dependencies requires blocked_by"))?;
                next.blocked_by = self.dependencies(blockers, Some(&id))?;
                if self.cycles(&next) {
                    return Err(error(
                        "TEAM_TASK_DEPENDENCY_CYCLE",
                        format!("task dependency cycle includes \"{id}\""),
                    ));
                }
            }
            Action::Complete => {
                may()?;
                if next.status != Status::InProgress {
                    return Err(transition("only an in-progress task can complete"));
                }
                next.status = Status::Completed;
            }
            Action::Reopen => {
                may()?;
                if next.status != Status::Completed {
                    return Err(transition("only a completed task can reopen"));
                }
                next.status = Status::Pending;
                next.owner = None;
            }
            Action::Reassign => self.reassign(&mut next, actor, change)?,
            Action::Delete => {
                may()?;
                self.delete(&mut next)?;
            }
        }
        next.revision += 1;
        if let Some(slot) = self.tasks.iter_mut().find(|t| t.id == id) {
            slot.clone_from(&next);
        }
        Ok(self.view_of(&next))
    }
}

/// Replaces what the change names: the title, the details, the write scopes.
fn edit(next: &mut Task, change: &Update) -> Result<(), TeamError> {
    if change.subject.is_none() && change.description.is_none() && change.write_scopes.is_none() {
        return Err(invalid(
            "task edit requires subject, description, or write_scopes",
        ));
    }
    if let Some(subject) = &change.subject {
        next.subject = text(subject, "subject", MAX_SUBJECT)?;
    }
    if let Some(description) = &change.description {
        next.description = text(description, "description", MAX_DESCRIPTION)?;
    }
    if let Some(scopes) = &change.write_scopes {
        next.write_scopes = write_scopes(scopes)?;
    }
    Ok(())
}
