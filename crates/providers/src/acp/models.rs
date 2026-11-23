//! Which model a coding agent uses. The agent says which models it offers when a session opens; the person's
//! choice is kept per agent in `~/.viv/acp-models.json` (with the last list the agent gave, so the picker can show
//! it before a session is open) and applied each time a session opens.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use vivianne_ai::paths::{viv_home_path, write_json_atomic};

use super::conn::{Conn, trace};
use super::info::{self, AgentModel};
use super::registry::Harness;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    preferred: Option<String>,
    #[serde(default)]
    known: Vec<AgentModel>,
}

fn path() -> std::path::PathBuf {
    viv_home_path(&["acp-models.json"])
}

fn read() -> HashMap<String, Entry> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn update(harness: &str, change: impl FnOnce(&mut Entry)) {
    let mut all = read();
    change(all.entry(harness.to_owned()).or_default());
    if let Err(e) = write_json_atomic(&path(), &all) {
        trace("harness", format!("couldn't save the model choice: {e}"));
    }
}

/// The model chosen for this agent, if the person chose one.
#[must_use]
pub fn preferred_model(harness: &str) -> Option<String> {
    read().remove(harness)?.preferred
}

pub(super) fn set_preferred(harness: &str, model: &str) {
    update(harness, |e| e.preferred = Some(model.to_owned()));
}

/// The models this agent offered last time it was running.
#[must_use]
pub fn known_models(harness: &str) -> Vec<AgentModel> {
    read().remove(harness).map(|e| e.known).unwrap_or_default()
}

/// Switches a running session to `model_id`.
pub(super) async fn switch(conn: &Conn, session: &str, model_id: &str) -> Result<(), String> {
    let option = info::model_option_id(&conn.info.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    let (method, params) = match option {
        Some(config_id) => (
            "session/set_config_option",
            json!({ "sessionId": session, "configId": config_id, "value": model_id }),
        ),
        None => (
            "session/set_model",
            json!({ "sessionId": session, "modelId": model_id }),
        ),
    };
    conn.request(method, params, Some(std::time::Duration::from_secs(30)))
        .await
        .map_err(|e| format!("the agent would not switch to {model_id}: {e}"))?;
    let mut info = conn.info.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    info.model_id = Some(model_id.to_owned());
    for o in &mut info.config_options {
        if o.get("id").and_then(serde_json::Value::as_str) == Some("model")
            || o.get("category").and_then(serde_json::Value::as_str) == Some("model")
        {
            o["currentValue"] = json!(model_id);
        }
    }
    Ok(())
}

/// A session has opened: remember what the agent offers, and put it on the model the person chose.
pub(super) async fn on_opened(conn: &Conn, harness: &Harness, session: &str) {
    let (offered, current) = {
        let info = conn.info.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (info::models_of(&info), info::current_model_id(&info))
    };
    if offered.is_empty() {
        return;
    }
    update(&harness.id, |e| e.known.clone_from(&offered));
    let Some(want) = preferred_model(&harness.id) else {
        return;
    };
    if Some(&want) == current.as_ref() || !offered.iter().any(|m| m.id == want) {
        return;
    }
    if let Err(e) = switch(conn, session, &want).await {
        trace("harness", e);
    }
}
