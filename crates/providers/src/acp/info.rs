//! What an agent says about its session (its commands, mode, options, title) and how full its context is.
//! Kept so the composer can offer its commands and the model picker its modes.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

/// A slash command the agent offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCommand {
    pub name: String,
    pub description: String,
    /// What to type after the name, when the command takes an argument.
    pub hint: Option<String>,
}

/// What an agent has said about its session so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionInfo {
    pub commands: Vec<AgentCommand>,
    /// The mode it is in (`plan`, `code`...), from `current_mode_update` or the option that has the `mode`
    /// category.
    pub mode: Option<String>,
    /// Its configuration options (model, mode, effort...), as it sent them.
    pub config_options: Vec<Value>,
    pub title: Option<String>,
}

/// A string field, trimmed, when it has anything in it.
pub(super) fn trimmed(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Folds one update into what is known. Bounded: an agent can't grow this without limit.
pub(super) fn apply_session_info(info: &mut SessionInfo, kind: &str, update: &Value) {
    match kind {
        "available_commands_update" => {
            info.commands = update
                .get("availableCommands")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|c| {
                            Some(AgentCommand {
                                name: trimmed(c.get("name"))?.chars().take(128).collect(),
                                description: trimmed(c.get("description")).unwrap_or_default().chars().take(1024).collect(),
                                hint: trimmed(c.pointer("/input/hint")).map(|h| h.chars().take(256).collect()),
                            })
                        })
                        .take(200)
                        .collect()
                })
                .unwrap_or_default();
        }
        "current_mode_update" => {
            if let Some(mode) = trimmed(update.get("currentModeId")) {
                info.mode = Some(mode.to_owned());
            }
        }
        "config_option_update" => {
            let list: Vec<Value> = update.get("configOptions").and_then(Value::as_array).map(|l| l.iter().take(100).cloned().collect()).unwrap_or_default();
            let mode = list.iter().find(|o| o.get("category").and_then(Value::as_str) == Some("mode")).and_then(|o| trimmed(o.get("currentValue")));
            if let Some(mode) = mode {
                info.mode = Some(mode.to_owned());
            }
            info.config_options = list;
        }
        "session_info_update" => match update.get("title") {
            // `null` clears the title; an absent one leaves it.
            Some(Value::Null) => info.title = None,
            Some(title) => {
                if let Some(title) = trimmed(Some(title)) {
                    info.title = Some(title.chars().take(1024).collect());
                }
            }
            None => {}
        },
        _ => {}
    }
}

/// How full each thread's agent says its context is: `(used, size)` in tokens, from the last `usage_update`.
/// The context meter reads it in place of an estimate from the messages Vivianne kept, which are not what the
/// agent holds.
static CONTEXT: Mutex<Option<HashMap<String, (u64, u64)>>> = Mutex::new(None);

pub(super) fn remember_context(thread: &str, used: u64, size: u64) {
    // A window of nothing is not a window: leave what was known.
    if size > 0 {
        CONTEXT.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(HashMap::new).insert(thread.to_owned(), (used, size));
    }
}

pub(super) fn forget_context(thread: &str) {
    if let Some(map) = CONTEXT.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        map.remove(thread);
    }
}

/// How full the thread's agent says its context is, in tokens: `(used, size)`.
#[must_use]
pub fn context_usage(thread: &str) -> Option<(u64, u64)> {
    CONTEXT.lock().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(thread).copied()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn commands_mode_options_and_title_are_kept_and_bounded() {
        let mut info = SessionInfo::default();
        apply_session_info(
            &mut info,
            "available_commands_update",
            &json!({"availableCommands": [{"name": " review ", "description": "Review", "input": {"hint": "files"}}, {"name": ""}]}),
        );
        assert_eq!(info.commands, vec![AgentCommand { name: "review".into(), description: "Review".into(), hint: Some("files".into()) }]);
        apply_session_info(&mut info, "config_option_update", &json!({"configOptions": [{"category": "mode", "currentValue": "plan"}]}));
        assert_eq!((info.mode.as_deref(), info.config_options.len()), (Some("plan"), 1));
        apply_session_info(&mut info, "current_mode_update", &json!({"currentModeId": "code"}));
        assert_eq!(info.mode.as_deref(), Some("code"));
        apply_session_info(&mut info, "session_info_update", &json!({"title": "Fixing a bug"}));
        assert_eq!(info.title.as_deref(), Some("Fixing a bug"));
        apply_session_info(&mut info, "session_info_update", &json!({"title": null}));
        assert_eq!(info.title, None);
        let many: Vec<Value> = (0..500).map(|n| json!({"name": format!("c{n}")})).collect();
        apply_session_info(&mut info, "available_commands_update", &json!({"availableCommands": many}));
        assert_eq!(info.commands.len(), 200);
    }

    #[test]
    fn context_usage_is_remembered_per_thread_and_a_zero_window_is_ignored() {
        remember_context("info-test", 10, 100);
        remember_context("info-test", 20, 0);
        assert_eq!(context_usage("info-test"), Some((10, 100)));
        forget_context("info-test");
        assert_eq!(context_usage("info-test"), None);
    }
}
