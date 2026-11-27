//! `team_task_create`, `team_task_list`, `team_task_get`, `team_task_update`.

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_tools::{Tool, ToolResult};

use super::board::{Action, Actor, Create, Owner, Status, TeamError, Update, error};
use super::store::Store;
use crate::orchestration::{Caller, Registry, caller};

/// The lead's id and name. No subagent can take it (`valid_name`).
const LEAD: &str = "lead";
const PAGE: u64 = 50;
const MAX_PAGE: u64 = 100;

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Create,
    List,
    Get,
    Update,
}

impl Kind {
    pub const ALL: [Self; 4] = [Self::Create, Self::List, Self::Get, Self::Update];
}

#[derive(Debug)]
pub struct TeamTool(pub Kind, pub Store);

fn actor(caller: &Caller) -> Actor {
    match &caller.member {
        Some(member) => Actor {
            owner: Owner {
                id: member.id.clone(),
                name: member.name.clone(),
            },
            lead: false,
        },
        None => Actor {
            owner: Owner {
                id: LEAD.to_owned(),
                name: LEAD.to_owned(),
            },
            lead: caller.depth == 0,
        },
    }
}

/// Who `name` is, for a reassignment: the lead, or a subagent that is still working.
fn member(caller: &Caller, name: &str) -> Result<Owner, TeamError> {
    if name == LEAD {
        return Ok(Owner {
            id: LEAD.to_owned(),
            name: LEAD.to_owned(),
        });
    }
    Registry::global()
        .get(&caller.thread, name)
        .filter(|handle| !handle.status().is_done())
        .map(|handle| Owner {
            id: handle.id.clone(),
            name: handle.name.clone(),
        })
        .ok_or_else(|| {
            error(
                "TEAM_MEMBER_NOT_FOUND",
                format!("\"{name}\" is not the lead or a subagent that is still working"),
            )
        })
}

fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str, TeamError> {
    args[key]
        .as_str()
        .ok_or_else(|| error("TEAM_INVALID_ARGUMENT", format!("{key} must be a string")))
}

fn strings(args: &Value, key: &str) -> Result<Option<Vec<String>>, TeamError> {
    let refused = || {
        error(
            "TEAM_INVALID_ARGUMENT",
            format!("{key} must be an array of strings"),
        )
    };
    match &args[key] {
        Value::Null => Ok(None),
        Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().map(str::to_owned).ok_or_else(refused))
            .collect::<Result<_, _>>()
            .map(Some),
        _ => Err(refused()),
    }
}

impl TeamTool {
    fn run(&self, caller: &Caller, args: &Value) -> Result<Value, TeamError> {
        let thread = &caller.thread;
        match self.0 {
            Kind::Create => {
                let request = Create {
                    subject: string(args, "subject")?.to_owned(),
                    description: string(args, "description")?.to_owned(),
                    blocked_by: strings(args, "blocked_by")?.unwrap_or_default(),
                    write_scopes: strings(args, "write_scopes")?.unwrap_or_default(),
                };
                let view = self.1.change(thread, |board| board.create(&request))?;
                Ok(json!(view))
            }
            Kind::Get => Ok(json!(self.1.read(thread)?.get(string(args, "task_id")?)?)),
            Kind::List => self.list(caller, args),
            Kind::Update => {
                let revision = args["expected_revision"].as_u64().ok_or_else(|| {
                    error(
                        "TEAM_INVALID_ARGUMENT",
                        "expected_revision must be a positive integer",
                    )
                })?;
                let mut change = Update::new(
                    string(args, "task_id")?,
                    revision,
                    Action::parse(string(args, "action")?)?,
                );
                change.subject = args["subject"].as_str().map(str::to_owned);
                change.description = args["description"].as_str().map(str::to_owned);
                change.blocked_by = strings(args, "blocked_by")?;
                change.write_scopes = strings(args, "write_scopes")?;
                if change.action == Action::Reassign
                    && let Some(name) = args["owner"].as_str()
                {
                    change.owner = Some(member(caller, name)?);
                }
                let actor = actor(caller);
                let view = self
                    .1
                    .change(thread, |board| board.update(&actor, &change))?;
                Ok(json!(view))
            }
        }
    }

    fn list(&self, caller: &Caller, args: &Value) -> Result<Value, TeamError> {
        let refused = |message: &str| error("TEAM_INVALID_ARGUMENT", message);
        let cursor = args["cursor"].as_u64().unwrap_or(0);
        let limit = args["limit"].as_u64().unwrap_or(PAGE);
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err(refused("limit must be an integer from 1 through 100"));
        }
        let status = match args["status"].as_str() {
            None => None,
            Some("pending") => Some(Status::Pending),
            Some("in_progress") => Some(Status::InProgress),
            Some("completed") => Some(Status::Completed),
            Some(_) => return Err(refused("status must be pending, in_progress or completed")),
        };
        let owner = args["owner"].as_str();
        let unowned = args["unowned"].as_bool();
        if owner.is_some() && unowned.is_some() {
            return Err(refused("owner and unowned filters cannot be combined"));
        }
        let ready = args["ready"].as_bool();
        let found: Vec<_> = self
            .1
            .read(&caller.thread)?
            .list()
            .into_iter()
            .map(|(view, _)| view)
            .filter(|t| status.is_none_or(|s| t.status == s))
            // By name, so a subagent that has ended can still be asked after.
            .filter(|t| owner.is_none_or(|o| t.owner.as_deref() == Some(o)))
            .filter(|t| unowned.is_none_or(|u| t.owner.is_none() == u))
            .filter(|t| ready.is_none_or(|r| t.ready == r))
            .collect();
        let total = found.len() as u64;
        let page: Vec<_> = found
            .into_iter()
            .skip(usize::try_from(cursor).unwrap_or(usize::MAX))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect();
        let mut out = json!({ "tasks": page });
        if cursor.saturating_add(limit) < total {
            out["nextCursor"] = json!(cursor + limit);
        }
        Ok(out)
    }
}

fn specification(kind: Kind) -> Value {
    let id = json!({ "type": "string", "description": "Shared task id." });
    let list = |description: &str| json!({ "type": "array", "items": { "type": "string" }, "description": description });
    match kind {
        Kind::Create => json!({
            "name": "team_task_create",
            "description": "Create one unowned pending task on the shared Team task board.",
            "parameters": { "type": "object", "properties": {
                "subject": { "type": "string", "description": "Concise task title." },
                "description": { "type": "string", "description": "Complete task details and acceptance criteria." },
                "blocked_by": list("Task ids that must complete first."),
                "write_scopes": list("Advisory workspace-relative file or directory prefixes this task expects to modify."),
            }, "required": ["subject", "description"] }
        }),
        Kind::List => json!({
            "name": "team_task_list",
            "description": "List shared tasks, including readiness, owner, revision, blockers, and write-scope warnings.",
            "parameters": { "type": "object", "properties": {
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "Optional exact status filter." },
                "owner": { "type": "string", "description": "Optional owner name filter; cannot be combined with unowned." },
                "unowned": { "type": "boolean", "description": "Filter by whether a task has no owner; cannot be combined with owner." },
                "ready": { "type": "boolean", "description": "Optional readiness filter." },
                "cursor": { "type": "integer", "description": "Zero-based result offset. Defaults to 0." },
                "limit": { "type": "integer", "description": "Number of rows, 1 through 100. Defaults to 50." },
            } }
        }),
        Kind::Get => json!({
            "name": "team_task_get",
            "description": "Read the complete latest value of one shared task before changing or executing it.",
            "parameters": { "type": "object", "properties": { "task_id": id }, "required": ["task_id"] }
        }),
        Kind::Update => json!({
            "name": "team_task_update",
            "description": "Compare-and-set a shared task action using the latest revision from team_task_get or team_task_list.",
            "parameters": { "type": "object", "properties": {
                "task_id": id,
                "expected_revision": { "type": "integer", "description": "Current task revision used as the precondition." },
                "action": { "type": "string", "enum": Action::NAMES, "description": "Task transition to apply." },
                "subject": { "type": "string", "description": "Replacement title for edit." },
                "description": { "type": "string", "description": "Replacement details for edit." },
                "blocked_by": list("Complete blocker list for set_dependencies."),
                "write_scopes": list("Replacement advisory write scopes for edit."),
                "owner": { "type": "string", "description": "For reassign, by the lead: `lead` or a working subagent's name; omit to unassign." },
            }, "required": ["task_id", "expected_revision", "action"] }
        }),
    }
}

#[async_trait]
impl Tool for TeamTool {
    fn name(&self) -> &'static str {
        match self.0 {
            Kind::Create => "team_task_create",
            Kind::List => "team_task_list",
            Kind::Get => "team_task_get",
            Kind::Update => "team_task_update",
        }
    }

    fn definition(&self) -> Option<Value> {
        Some(specification(self.0))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(caller) = caller() else {
            return ToolResult::failure("the task board works inside a conversation's own agent.");
        };
        match self.run(&caller, args) {
            Ok(value) => ToolResult::ok(value.to_string()),
            Err(why) => ToolResult::failure(why.to_string()),
        }
    }
}
