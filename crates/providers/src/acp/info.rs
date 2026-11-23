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
    /// The models it offers when it lists them apart from its options (`session/new`'s `models`), and the one it
    /// is using.
    pub models: Vec<AgentModel>,
    pub model_id: Option<String>,
    pub title: Option<String>,
}

/// A model an agent can be switched to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentModel {
    pub id: String,
    pub name: String,
}

/// The agent's model option, when it offers one through its configuration options.
fn model_option(info: &SessionInfo) -> Option<&Value> {
    info.config_options.iter().find(|o| {
        o.get("category").and_then(Value::as_str) == Some("model")
            || o.get("id").and_then(Value::as_str) == Some("model")
    })
}

/// The id of the configuration option that picks the model, when the agent has one: switching goes through it.
#[must_use]
pub fn model_option_id(info: &SessionInfo) -> Option<String> {
    trimmed(model_option(info)?.get("id")).map(str::to_owned)
}

/// Every model the agent offers, from its model option or its model list.
#[must_use]
pub fn models_of(info: &SessionInfo) -> Vec<AgentModel> {
    if let Some(choices) =
        model_option(info).and_then(|o| o.get("options").and_then(Value::as_array))
    {
        let flat = choices
            .iter()
            .flat_map(|c| match c.get("options").and_then(Value::as_array) {
                Some(group) => group.iter().collect::<Vec<_>>(),
                None => vec![c],
            });
        let list: Vec<AgentModel> = flat
            .filter_map(|c| {
                let id = trimmed(c.get("value"))?;
                Some(AgentModel {
                    id: id.to_owned(),
                    name: trimmed(c.get("name"))
                        .unwrap_or(id)
                        .chars()
                        .take(80)
                        .collect(),
                })
            })
            .take(200)
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    info.models.clone()
}

/// The id of the model in use.
#[must_use]
pub fn current_model_id(info: &SessionInfo) -> Option<String> {
    model_option(info)
        .and_then(|o| trimmed(o.get("currentValue")))
        .map(str::to_owned)
        .or_else(|| info.model_id.clone())
}

/// Takes in the answer to `session/new` (or a resume): its options and its list of models.
pub(super) fn apply_opened(info: &mut SessionInfo, result: &Value) {
    if let Some(list) = result.get("configOptions").and_then(Value::as_array) {
        info.config_options = list.iter().take(100).cloned().collect();
    }
    if let Some(models) = result.get("models") {
        info.models = models
            .get("availableModels")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(|m| {
                        let id = trimmed(m.get("modelId").or_else(|| m.get("id")))?;
                        Some(AgentModel {
                            id: id.to_owned(),
                            name: trimmed(m.get("name"))
                                .unwrap_or(id)
                                .chars()
                                .take(80)
                                .collect(),
                        })
                    })
                    .take(200)
                    .collect()
            })
            .unwrap_or_default();
        info.model_id = trimmed(models.get("currentModelId")).map(str::to_owned);
    }
}

/// The model the agent says it is using, as a name a person would recognise: the current value of its `model`
/// option, shown by the option's own label for that value when it has one. `None` when it has not said.
#[must_use]
pub fn current_model(info: &SessionInfo) -> Option<String> {
    let option = info.config_options.iter().find(|o| {
        o.get("category").and_then(Value::as_str) == Some("model")
            || o.get("id").and_then(Value::as_str) == Some("model")
    })?;
    let value = trimmed(option.get("currentValue"))?;
    // Options are a flat list, or groups of them.
    let choices = option.get("options").and_then(Value::as_array)?;
    let flat = choices
        .iter()
        .flat_map(|c| match c.get("options").and_then(Value::as_array) {
            Some(group) => group.iter().collect::<Vec<_>>(),
            None => vec![c],
        });
    let label = flat
        .filter(|c| trimmed(c.get("value")) == Some(value))
        .find_map(|c| trimmed(c.get("name")));
    // Kept short and clean: it becomes part of a usage key, where a slash would split it.
    Some(
        label
            .unwrap_or(value)
            .replace('/', "-")
            .chars()
            .take(80)
            .collect(),
    )
}

/// A string field, trimmed, when it has anything in it.
pub(super) fn trimmed(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
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
                                description: trimmed(c.get("description"))
                                    .unwrap_or_default()
                                    .chars()
                                    .take(1024)
                                    .collect(),
                                hint: trimmed(c.pointer("/input/hint"))
                                    .map(|h| h.chars().take(256).collect()),
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
            let list: Vec<Value> = update
                .get("configOptions")
                .and_then(Value::as_array)
                .map(|l| l.iter().take(100).cloned().collect())
                .unwrap_or_default();
            let mode = list
                .iter()
                .find(|o| o.get("category").and_then(Value::as_str) == Some("mode"))
                .and_then(|o| trimmed(o.get("currentValue")));
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
        CONTEXT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(thread.to_owned(), (used, size));
    }
}

pub(super) fn forget_context(thread: &str) {
    if let Some(map) = CONTEXT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        map.remove(thread);
    }
}

/// How full the thread's agent says its context is, in tokens: `(used, size)`.
#[must_use]
pub fn context_usage(thread: &str) -> Option<(u64, u64)> {
    CONTEXT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()?
        .get(thread)
        .copied()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_model_in_use_is_read_from_the_model_option_by_its_label_flat_or_grouped() {
        let mut info = SessionInfo::default();
        assert_eq!(current_model(&info), None);
        info.config_options = vec![
            json!({"id": "mode", "category": "mode", "currentValue": "code"}),
            json!({"id": "model", "category": "model", "currentValue": "opus", "options": [{"value": "sonnet", "name": "Sonnet 4.5"}, {"value": "opus", "name": "Opus 4.5"}]}),
        ];
        assert_eq!(current_model(&info).as_deref(), Some("Opus 4.5"));
        info.config_options = vec![
            json!({"id": "model", "currentValue": "gpt-5/high", "options": [{"group": "g", "options": [{"value": "gpt-5/high", "name": "GPT-5 / high"}]}]}),
        ];
        assert_eq!(current_model(&info).as_deref(), Some("GPT-5 - high"));
        info.config_options =
            vec![json!({"id": "model", "currentValue": "unlisted", "options": []})];
        assert_eq!(current_model(&info).as_deref(), Some("unlisted"));
    }

    #[test]
    fn commands_mode_options_and_title_are_kept_and_bounded() {
        let mut info = SessionInfo::default();
        apply_session_info(
            &mut info,
            "available_commands_update",
            &json!({"availableCommands": [{"name": " review ", "description": "Review", "input": {"hint": "files"}}, {"name": ""}]}),
        );
        assert_eq!(
            info.commands,
            vec![AgentCommand {
                name: "review".into(),
                description: "Review".into(),
                hint: Some("files".into())
            }]
        );
        apply_session_info(
            &mut info,
            "config_option_update",
            &json!({"configOptions": [{"category": "mode", "currentValue": "plan"}]}),
        );
        assert_eq!(
            (info.mode.as_deref(), info.config_options.len()),
            (Some("plan"), 1)
        );
        apply_session_info(
            &mut info,
            "current_mode_update",
            &json!({"currentModeId": "code"}),
        );
        assert_eq!(info.mode.as_deref(), Some("code"));
        apply_session_info(
            &mut info,
            "session_info_update",
            &json!({"title": "Fixing a bug"}),
        );
        assert_eq!(info.title.as_deref(), Some("Fixing a bug"));
        apply_session_info(&mut info, "session_info_update", &json!({"title": null}));
        assert_eq!(info.title, None);
        let many: Vec<Value> = (0..500).map(|n| json!({"name": format!("c{n}")})).collect();
        apply_session_info(
            &mut info,
            "available_commands_update",
            &json!({"availableCommands": many}),
        );
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

    #[test]
    fn the_models_an_agent_offers_are_read_from_either_of_its_shapes() {
        let mut info = SessionInfo::default();
        apply_opened(
            &mut info,
            &json!({"models": {"currentModelId": "opus", "availableModels": [
                {"modelId": "opus", "name": "Opus"}, {"modelId": "sonnet", "name": "Sonnet"}]}}),
        );
        let ids = |i: &SessionInfo| models_of(i).into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&info), ["opus", "sonnet"]);
        assert_eq!(current_model_id(&info).as_deref(), Some("opus"));
        assert!(
            model_option_id(&info).is_none(),
            "no option: set_model is used"
        );

        apply_opened(
            &mut info,
            &json!({"configOptions": [{"id": "model", "category": "model", "currentValue": "gpt-5",
                "options": [{"value": "gpt-5", "name": "GPT-5"}, {"value": "o3", "name": "o3"}]}]}),
        );
        assert_eq!(ids(&info), ["gpt-5", "o3"]);
        assert_eq!(current_model_id(&info).as_deref(), Some("gpt-5"));
        assert_eq!(model_option_id(&info).as_deref(), Some("model"));
    }
}
