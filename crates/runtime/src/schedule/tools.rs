//! `schedule_create`, `schedule_list`, `schedule_view` and `schedule_edit`: the model's way to schedule time-based
//! work. Creating and editing ask for approval like any change; listing and viewing only look. A subagent cannot
//! schedule: it has no conversation of its own to be delivered to.

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_tools::{Tool, ToolResult};

use super::domain::{self, CreateSpec, ScheduleError, Target, Timing, iso};
use super::store::{Entry, Patch, Store, StoreError};
use crate::orchestration::{Caller, caller};

fn now() -> i64 {
    i64::try_from(crate::workspace_context::now_ms()).unwrap_or(i64::MAX)
}

fn failure(code: &str, message: &str) -> ToolResult {
    ToolResult::error(json!({ "code": code, "message": message }).to_string())
}

fn store_failure(e: &StoreError) -> ToolResult {
    let code = match e {
        StoreError::NotFound(_) => "schedule_not_found",
        StoreError::Conflict => "schedule_conflict",
        StoreError::Invalid(inner) => inner.code(),
        _ => "internal_error",
    };
    failure(code, &e.to_string())
}

fn describe(entry: &Entry) -> Value {
    let t = &entry.task;
    json!({
        "id": t.id,
        "title": t.title,
        "prompt": t.prompt,
        "timing": t.timing,
        "target": t.target,
        "active": t.active,
        "next_at": t.active.then(|| iso(t.scheduled_at)),
        "updated_at": t.updated_at,
        "last_delivery": entry.deliveries.last().map(|d| json!({ "occurrence_at": iso(d.occurrence_at), "session_id": d.session_id })),
    })
}

/// Reads the timing selector out of the arguments: exactly one of the ways to say when.
fn timing_of(args: &Value) -> Result<Timing, ScheduleError> {
    let selected: Vec<&str> = ["after_seconds", "at", "every_seconds", "daily", "weekly", "cron"]
        .into_iter()
        .filter(|k| !args[*k].is_null())
        .collect();
    let [which] = selected.as_slice() else {
        return Err(ScheduleError::InvalidSelector(
            "give exactly one of after_seconds, at, every_seconds, daily, weekly or cron".into(),
        ));
    };
    let bad = || ScheduleError::InvalidSelector(format!("{which} is not written as expected"));
    let text = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or_else(bad);
    let value = &args[*which];
    Ok(match *which {
        "after_seconds" => Timing::After { after_seconds: value.as_u64().ok_or_else(bad)? },
        "every_seconds" => Timing::Every { every_seconds: value.as_u64().ok_or_else(bad)? },
        "at" => match value.as_str() {
            Some(at) => Timing::At { at: at.to_owned() },
            None => Timing::AtLocal { date: text(value, "date")?, time: text(value, "time")?, time_zone: text(value, "time_zone")? },
        },
        "daily" => Timing::Daily { time: text(value, "time")?, time_zone: text(value, "time_zone")? },
        "weekly" => Timing::Weekly {
            time: text(value, "time")?,
            time_zone: text(value, "time_zone")?,
            weekdays: value["weekdays"]
                .as_array()
                .ok_or_else(bad)?
                .iter()
                .map(|d| d.as_u64().and_then(|d| u8::try_from(d).ok()).ok_or_else(bad))
                .collect::<Result<_, _>>()?,
        },
        _ => Timing::Cron { expression: text(value, "expression")?, time_zone: text(value, "time_zone")? },
    })
}

/// Where the task goes: `this` conversation (the default inside one), a `new` one each run, or `new_once`.
fn target_of(args: &Value, from: Option<&Caller>) -> Result<Target, ScheduleError> {
    let create = || CreateSpec {
        cwd: from.map(|c| c.folder.display().to_string()),
        profile: None,
    };
    match args["target"].as_str() {
        None | Some("this") => match from {
            Some(c) => Ok(Target::Session { session_id: c.thread.clone() }),
            None => Ok(Target::New { create: create() }),
        },
        Some("new") => Ok(Target::New { create: create() }),
        Some("new_once") => Ok(Target::NewOnce { session_id: String::new(), create: create() }),
        Some(other) => Err(ScheduleError::InvalidSelector(format!("target is this, new or new_once, not `{other}`"))),
    }
}

/// A subagent has no conversation of its own to deliver to.
fn refuse_subagent(from: Option<&Caller>) -> Option<ToolResult> {
    from.filter(|c| c.depth > 0)
        .map(|_| failure("subagent_session", "a subagent cannot schedule tasks"))
}

fn timing_properties() -> Value {
    let zoned = json!({ "type": "object", "properties": { "time": { "type": "string", "description": "HH:MM:SS" }, "time_zone": { "type": "string", "description": "IANA name, like Europe/Paris" } }, "required": ["time", "time_zone"] });
    let mut weekly = zoned.clone();
    weekly["properties"]["weekdays"] = json!({ "type": "array", "items": { "type": "integer" }, "description": "1 Monday ... 7 Sunday" });
    json!({
        "after_seconds": { "type": "integer", "description": "Once, this many seconds from now" },
        "at": { "description": "Once: an RFC 3339 instant with offset, or {date, time, time_zone}" },
        "every_seconds": { "type": "integer", "description": "Repeating, at least 60" },
        "daily": zoned.clone(),
        "weekly": weekly,
        "cron": { "type": "object", "properties": { "expression": { "type": "string", "description": "Five fields: minute hour day month weekday (0 or 7 = Sunday)" }, "time_zone": { "type": "string" } }, "required": ["expression", "time_zone"] },
    })
}

#[derive(Debug, Clone)]
pub struct ScheduleCreate(pub Store);

#[async_trait]
impl Tool for ScheduleCreate {
    fn name(&self) -> &str {
        "schedule_create"
    }

    fn definition(&self) -> Option<Value> {
        let mut properties = timing_properties();
        properties["title"] = json!({ "type": "string", "description": "A short name" });
        properties["prompt"] = json!({ "type": "string", "description": "What to deliver when it fires" });
        properties["target"] = json!({ "type": "string", "enum": ["this", "new", "new_once"], "description": "this conversation (default), a new one each run, or one new one reused" });
        Some(json!({
            "name": "schedule_create",
            "description": "Deliver an instruction to a conversation later or on a repeating schedule (time-based; for folder-watching use create_automation). Give exactly one of after_seconds, at, every_seconds, daily, weekly or cron. Ask the user for the time zone when it matters and is unknown.",
            "parameters": { "type": "object", "properties": properties, "required": ["title", "prompt"] }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let from = caller();
        if let Some(refused) = refuse_subagent(from.as_ref()) {
            return refused;
        }
        let made = timing_of(args).and_then(|timing| {
            let target = target_of(args, from.as_ref())?;
            domain::create(
                &uuid::Uuid::new_v4().to_string(),
                args["title"].as_str().unwrap_or(""),
                args["prompt"].as_str().unwrap_or(""),
                &timing,
                target,
                now(),
            )
        });
        match made {
            Ok(task) => match self.0.add(task) {
                Ok(entry) => ToolResult::ok(describe(&entry).to_string()),
                Err(e) => store_failure(&e),
            },
            Err(e) => failure(e.code(), &e.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleList(pub Store);

#[async_trait]
impl Tool for ScheduleList {
    fn name(&self) -> &str {
        "schedule_list"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "schedule_list",
            "description": "List scheduled tasks, soonest first.",
            "parameters": { "type": "object", "properties": {
                "active": { "type": "boolean" }, "offset": { "type": "integer" }, "limit": { "type": "integer" }
            } }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let offset = args["offset"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
        let limit = args["limit"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(20);
        match self.0.list(args["active"].as_bool(), offset, limit) {
            Ok((items, total)) => ToolResult::ok(
                json!({ "items": items.iter().map(describe).collect::<Vec<_>>(), "shown": items.len(), "total": total, "offset": offset }).to_string(),
            ),
            Err(e) => store_failure(&e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleView(pub Store);

#[async_trait]
impl Tool for ScheduleView {
    fn name(&self) -> &str {
        "schedule_view"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "schedule_view",
            "description": "One scheduled task in full.",
            "parameters": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        match self.0.view(args["id"].as_str().unwrap_or("")) {
            Ok(entry) => ToolResult::ok(describe(&entry).to_string()),
            Err(e) => store_failure(&e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleEdit(pub Store);

#[async_trait]
impl Tool for ScheduleEdit {
    fn name(&self) -> &str {
        "schedule_edit"
    }

    fn definition(&self) -> Option<Value> {
        let mut patch = timing_properties();
        patch["title"] = json!({ "type": "string" });
        patch["prompt"] = json!({ "type": "string" });
        patch["target"] = json!({ "type": "string", "enum": ["this", "new", "new_once"] });
        patch["active"] = json!({ "type": "boolean", "description": "false pauses; true takes it up again from now" });
        Some(json!({
            "name": "schedule_edit",
            "description": "Change or delete a scheduled task. Pass the updated_at you read so a change made meanwhile is not overwritten.",
            "parameters": { "type": "object", "properties": {
                "id": { "type": "string" },
                "action": { "type": "string", "enum": ["update", "delete"] },
                "patch": { "type": "object", "properties": patch },
                "expected_updated_at": { "type": "integer" }
            }, "required": ["id", "action"] }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let from = caller();
        if let Some(refused) = refuse_subagent(from.as_ref()) {
            return refused;
        }
        let id = args["id"].as_str().unwrap_or("");
        let expected = args["expected_updated_at"].as_i64();
        match args["action"].as_str() {
            Some("delete") => match self.0.delete(id, expected) {
                Ok(()) => ToolResult::ok(json!({ "id": id, "deleted": true }).to_string()),
                Err(e) => store_failure(&e),
            },
            Some("update") => {
                let p = &args["patch"];
                let patch = patch_of(p, from.as_ref());
                match patch {
                    Ok(patch) => match self.0.edit(id, &patch, expected, now()) {
                        Ok(entry) => ToolResult::ok(describe(&entry).to_string()),
                        Err(e) => store_failure(&e),
                    },
                    Err(e) => failure(e.code(), &e.to_string()),
                }
            }
            _ => failure("invalid_selector", "action is update or delete"),
        }
    }
}

fn patch_of(p: &Value, from: Option<&Caller>) -> Result<Patch, ScheduleError> {
    let says_when = ["after_seconds", "at", "every_seconds", "daily", "weekly", "cron"]
        .iter()
        .any(|k| !p[*k].is_null());
    Ok(Patch {
        title: p["title"].as_str().map(str::to_owned),
        prompt: p["prompt"].as_str().map(str::to_owned),
        timing: says_when.then(|| timing_of(p)).transpose()?,
        target: p["target"].is_string().then(|| target_of(p, from)).transpose()?,
        active: p["active"].as_bool(),
    })
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
