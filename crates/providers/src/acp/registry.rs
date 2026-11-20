//! Which coding agents exist and how to start them, from the public Agent Client Protocol registry
//! (cached for a day in `~/.viv/acp-registry.json`), with a built-in list for when it can't be fetched.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use vivianne_ai::paths::{viv_home_path, write_json_atomic};

const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
const CACHE_FOR: Duration = Duration::from_hours(24);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// A coding agent Vivianne can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    /// The launcher (`npx` or `uvx`) is on this computer.
    pub available: bool,
    /// Its package has been fetched and started here at least once, so a turn won't wait on a download.
    pub ready: bool,
    /// What starts it: `npx` or `uvx`.
    pub launcher: String,
    pub(super) args: Vec<String>,
    pub(super) env: HashMap<String, String>,
}

impl Harness {
    /// The launcher's file on this computer (`npx.cmd` on Windows), if it is installed.
    #[must_use]
    pub fn program(&self) -> Option<PathBuf> {
        which::which(&self.launcher).ok()
    }
}

/// The agents people ask for first, at the versions the registry listed when this was written: used when the
/// registry can't be fetched. (id, name, description, package, extra arguments.)
const FALLBACK: &[(&str, &str, &str, &str, &[&str])] = &[
    ("claude-acp", "Claude Code", "Anthropic's Claude Code, through its ACP adapter.", "@agentclientprotocol/claude-agent-acp@0.99.1", &[]),
    ("codex-acp", "Codex", "OpenAI's Codex CLI, through its ACP adapter.", "@agentclientprotocol/codex-acp@2.5.0", &[]),
    ("gemini", "Gemini CLI", "Google's Gemini CLI.", "@google/gemini-cli@0.66.0", &["--acp"]),
    ("github-copilot-cli", "Copilot", "GitHub Copilot CLI.", "@github/copilot@1.0.105", &["--acp"]),
    ("qwen-code", "Qwen Code", "Qwen Code CLI.", "@qwen-code/qwen-code@0.26.4", &["--acp"]),
];

/// The registry's agents most people use, first in the list.
const FEATURED: &[&str] = &[
    "claude-acp", "codex-acp", "gemini", "github-copilot-cli", "cursor", "opencode", "grok-build", "qwen-code", "goose", "cline",
];

fn ready_path() -> PathBuf {
    viv_home_path(&["acp-ready.json"])
}

static READY_WRITE: Mutex<()> = Mutex::new(());

fn ready_ids() -> Vec<String> {
    std::fs::read_to_string(ready_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Remembers that this agent started and answered `initialize` here.
pub(super) fn mark_ready(id: &str) {
    // Two agents starting at once would otherwise each write a list missing the other.
    let _guard = READY_WRITE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut ids = ready_ids();
    if !ids.iter().any(|known| known == id) {
        ids.push(id.to_owned());
        let _ = write_json_atomic(&ready_path(), &ids);
    }
}

/// One registry entry as something that can run. Only `npx` and `uvx` distributions run here; downloaded
/// binaries are left out.
fn from_registry(agent: &Value) -> Option<Harness> {
    let id = agent.get("id")?.as_str()?.to_owned();
    let dist = agent.get("distribution")?;
    let (launcher, package, extra, env) = if let Some(n) = dist.get("npx") {
        ("npx", n.get("package")?.as_str()?, n.get("args"), n.get("env"))
    } else if let Some(u) = dist.get("uvx") {
        ("uvx", u.get("package")?.as_str()?, u.get("args"), u.get("env"))
    } else {
        return None;
    };
    let mut args = if launcher == "npx" { vec!["-y".to_owned(), package.to_owned()] } else { vec![package.to_owned()] };
    args.extend(extra.and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)));
    let env = env
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))).collect())
        .unwrap_or_default();
    let text = |key: &str| agent.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Harness {
        name: text("name").unwrap_or_else(|| id.clone()),
        description: text("description").unwrap_or_default(),
        icon: text("icon"),
        available: which::which(launcher).is_ok(),
        ready: false,
        launcher: launcher.to_owned(),
        args,
        env,
        id,
    })
}

fn fallback() -> Vec<Harness> {
    let npx = which::which("npx").is_ok();
    FALLBACK
        .iter()
        .map(|(id, name, description, package, extra)| Harness {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            description: (*description).to_owned(),
            icon: None,
            available: npx,
            ready: false,
            launcher: "npx".to_owned(),
            args: ["-y", package].iter().chain(extra.iter()).map(|s| (*s).to_owned()).collect(),
            env: HashMap::new(),
        })
        .collect()
}

fn registry_cache() -> PathBuf {
    viv_home_path(&["acp-registry.json"])
}

/// The registry's text: the cache when it is under a day old, else fetched (and cached); an older cache when
/// the fetch fails.
async fn registry_text() -> Option<String> {
    let cache = registry_cache();
    let fresh = std::fs::metadata(&cache)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < CACHE_FOR);
    if fresh && let Ok(text) = std::fs::read_to_string(&cache) {
        return Some(text);
    }
    let fetched = async {
        let response = vivianne_ai::ai::http::client().get(REGISTRY_URL).timeout(FETCH_TIMEOUT).send().await.ok()?;
        response.error_for_status().ok()?.text().await.ok()
    }
    .await;
    match fetched {
        Some(text) => {
            // Only a body that parses is worth keeping: an error page cached for a day would hide the agents.
            if serde_json::from_str::<Value>(&text).is_ok() {
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&cache, &text);
            }
            Some(text)
        }
        None => std::fs::read_to_string(&cache).ok(),
    }
}

/// Every agent that can run here, the common ones first.
pub async fn harnesses() -> Vec<Harness> {
    let mut list = registry_text().await.map(|text| parse(&text)).unwrap_or_default();
    if list.is_empty() {
        list = fallback();
    }
    let ready = ready_ids();
    for harness in &mut list {
        harness.ready = ready.contains(&harness.id);
    }
    list.sort_by_key(|h| (FEATURED.iter().position(|f| *f == h.id).unwrap_or(usize::MAX), h.name.to_lowercase()));
    list
}

fn parse(text: &str) -> Vec<Harness> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("agents").and_then(Value::as_array).cloned())
        .map(|agents| agents.iter().filter_map(from_registry).collect())
        .unwrap_or_default()
}

/// The agent with this id.
pub async fn find(id: &str) -> Result<Harness, String> {
    harnesses().await.into_iter().find(|h| h.id == id).ok_or_else(|| format!("No coding agent called '{id}'."))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn registry_entries_become_commands_and_binary_only_ones_are_left_out() {
        let text = json!({"agents": [
            {"id": "gemini", "name": "Gemini CLI", "distribution": {"npx": {"package": "@google/gemini-cli@0.66.0", "args": ["--acp"]}}},
            {"id": "tool", "name": "Tool", "distribution": {"uvx": {"package": "tool==1.0", "env": {"A": "b"}}}},
            {"id": "goose", "name": "goose", "distribution": {"binary": {}}},
            {"name": "no id"}
        ]})
        .to_string();
        let list = parse(&text);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].launcher.as_str(), list[0].args.clone()), ("npx", vec!["-y".into(), "@google/gemini-cli@0.66.0".into(), "--acp".into()]));
        assert_eq!((list[1].launcher.as_str(), list[1].args.clone(), list[1].env["A"].as_str()), ("uvx", vec!["tool==1.0".into()], "b"));
    }

    #[test]
    fn the_built_in_list_runs_through_npx() {
        let list = fallback();
        assert!(list.iter().all(|h| h.launcher == "npx" && h.args[0] == "-y"));
        assert!(list.iter().any(|h| h.id == "claude-acp"));
    }
}
