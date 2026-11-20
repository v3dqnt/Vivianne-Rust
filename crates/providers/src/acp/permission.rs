//! What the agent asks of us: permission to act. Reading never asks; plan mode refuses a change; bypass mode
//! allows; everything else asks the user.

use serde_json::{Value, json};

/// What to do with a permission request, by the kind of tool call behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Disposition {
    Allow,
    Ask,
    Deny,
}

/// Reading never asks: a read, a search or a thought changes nothing. Past that, plan mode refuses (the agent
/// may still fetch from the web), bypass mode allows, and everything else asks. A request to switch the
/// agent's own mode is its plan changing, not a change to anything of the user's, so plan mode asks about it
/// instead of refusing.
pub(super) fn disposition(kind: &str, plan_mode: bool, bypass: bool) -> Disposition {
    match kind {
        "read" | "search" | "think" => Disposition::Allow,
        "fetch" if plan_mode => Disposition::Allow,
        "switch_mode" => {
            if bypass { Disposition::Allow } else { Disposition::Ask }
        }
        _ if plan_mode => Disposition::Deny,
        _ if bypass => Disposition::Allow,
        _ => Disposition::Ask,
    }
}

/// The tool name the chat draws for an agent's tool call kind.
pub(super) fn tool_name(kind: &str) -> &'static str {
    match kind {
        "read" => "read",
        "edit" => "edit",
        "delete" | "move" => "write",
        "search" => "grep",
        "execute" => "bash",
        "fetch" => "web_fetch",
        "think" => "think",
        _ => "tool",
    }
}

/// A permission request, read the same whichever version sent it.
#[derive(Debug)]
pub(super) struct PermissionAsk {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// The agent's word for the kind of call, if it gave one.
    pub kind: Option<String>,
    pub title: Option<String>,
    pub options: Vec<Value>,
}

/// The id of the tool call a permission request is about, in either version's shape.
pub(super) fn call_id(params: &Value) -> Option<&str> {
    params
        .pointer("/toolCall/toolCallId")
        .or_else(|| params.pointer("/subject/toolCall/toolCallId"))
        .or_else(|| params.pointer("/subject/toolCallId"))
        .and_then(Value::as_str)
}

/// v1 puts the tool call at the top of the request; v2 puts it under `subject`, which is either a tool call or
/// a command about to run. `known_kind` is what the agent said the call was when it announced it, for agents
/// that leave the kind off the request (without it a write could not be told from a read).
pub(super) fn permission_ask(params: &Value, known_kind: Option<String>) -> PermissionAsk {
    let subject = params.get("subject");
    let call = match subject.and_then(|s| s.get("type")).and_then(Value::as_str) {
        Some("tool_call") => subject.and_then(|s| s.get("toolCall")).cloned().unwrap_or(Value::Null),
        Some("command") => {
            let s = subject.cloned().unwrap_or(Value::Null);
            json!({
                "toolCallId": s.get("toolCallId"),
                "kind": "execute",
                "title": s.get("command"),
                "rawInput": { "command": s.get("command"), "cwd": s.get("cwd") },
            })
        }
        _ => params.get("toolCall").cloned().unwrap_or(Value::Null),
    };
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    let kind = text(call.get("kind")).or(known_kind);
    let title = text(call.get("title")).or_else(|| text(params.get("title")));
    let args = call.get("rawInput").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({ "title": title }));
    PermissionAsk {
        id: text(call.get("toolCallId")).unwrap_or_default(),
        name: tool_name(kind.as_deref().unwrap_or_default()).to_owned(),
        args,
        kind,
        title,
        options: params.get("options").and_then(Value::as_array).cloned().unwrap_or_default(),
    }
}

pub(super) fn cancelled_outcome() -> Value {
    json!({ "outcome": { "outcome": "cancelled" } })
}

/// The option of the first kind in `kinds` that the agent offered, in the order of `kinds` (not of the agent's
/// own list).
pub(super) fn pick_option(options: &[Value], kinds: &[&str]) -> Option<Value> {
    kinds.iter().find_map(|kind| options.iter().find(|o| o.get("kind").and_then(Value::as_str) == Some(kind)).and_then(|o| o.get("optionId").cloned()))
}

/// The answer to a permission request. Approval is for this call only: `allow_always` would grant the agent a
/// standing permission the user never gave, so it is never picked, and an agent that offers nothing else is
/// told the request was cancelled.
pub(super) fn permission_outcome(options: &[Value], approved: bool) -> Value {
    let option = if approved { pick_option(options, &["allow_once"]) } else { pick_option(options, &["reject_once", "reject_always"]) };
    match option {
        Some(option) => json!({ "outcome": { "outcome": "selected", "optionId": option } }),
        None => cancelled_outcome(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered() -> Vec<Value> {
        vec![
            json!({"kind": "allow_always", "optionId": "a"}),
            json!({"kind": "allow_once", "optionId": "o"}),
            json!({"kind": "reject_always", "optionId": "ra"}),
            json!({"kind": "reject_once", "optionId": "ro"}),
        ]
    }

    #[test]
    fn approval_never_picks_a_standing_grant_and_follows_our_order() {
        let options = offered();
        assert_eq!(pick_option(&options, &["allow_once"]), Some(json!("o")), "allow_once wins even when allow_always is listed first");
        assert_eq!(pick_option(&options, &["reject_once", "reject_always"]), Some(json!("ro")));
        let only_always = vec![json!({"kind": "allow_always", "optionId": "a"})];
        assert_eq!(pick_option(&only_always, &["allow_once"]), None);
        assert_eq!(permission_outcome(&only_always, true), cancelled_outcome(), "no one-shot option: cancel instead of granting forever");
        assert_eq!(permission_outcome(&offered(), true)["outcome"]["optionId"], "o");
        assert_eq!(permission_outcome(&offered(), false)["outcome"]["optionId"], "ro");
    }

    #[test]
    fn a_permission_is_allowed_asked_or_refused_by_the_kind_of_call() {
        use Disposition::{Allow, Ask, Deny};
        assert_eq!(disposition("read", false, false), Allow);
        assert_eq!(disposition("search", true, false), Allow);
        assert_eq!(disposition("edit", false, false), Ask);
        assert_eq!(disposition("execute", false, true), Allow, "bypass allows");
        assert_eq!(disposition("edit", true, false), Deny, "plan mode refuses a change");
        assert_eq!(disposition("fetch", true, false), Allow, "plan mode may still fetch");
        assert_eq!(disposition("switch_mode", true, false), Ask);
        assert_eq!(disposition("other", false, false), Ask);
    }

    #[test]
    fn a_permission_request_reads_the_same_in_either_version() {
        let v1 = json!({"toolCall": {"toolCallId": "t1", "kind": "edit", "title": "Edit a.rs", "rawInput": {"path": "a.rs"}}, "options": []});
        let v2 = json!({"subject": {"type": "tool_call", "toolCall": {"toolCallId": "t1", "kind": "edit", "title": "Edit a.rs", "rawInput": {"path": "a.rs"}}}, "options": []});
        for params in [v1, v2] {
            let ask = permission_ask(&params, None);
            assert_eq!((ask.id.as_str(), ask.name.as_str(), ask.title.as_deref()), ("t1", "edit", Some("Edit a.rs")));
            assert_eq!(ask.args["path"], "a.rs");
            assert_eq!(call_id(&params), Some("t1"));
        }
    }

    #[test]
    fn a_command_subject_is_asked_as_a_shell_command_and_a_missing_kind_uses_what_was_announced() {
        let command = json!({"subject": {"type": "command", "toolCallId": "c1", "command": "cargo test", "cwd": "/p"}, "options": []});
        let ask = permission_ask(&command, None);
        assert_eq!((ask.name.as_str(), ask.kind.as_deref()), ("bash", Some("execute")));
        assert_eq!(ask.args["command"], "cargo test");
        let no_kind = json!({"toolCall": {"toolCallId": "t"}, "options": []});
        assert_eq!(permission_ask(&no_kind, Some("edit".into())).kind.as_deref(), Some("edit"));
        assert_eq!(permission_ask(&no_kind, None).name, "tool");
    }
}
