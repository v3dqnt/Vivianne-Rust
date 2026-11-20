//! Which coding agents exist and how to start them, from the public Agent Client Protocol registry
//! (cached for a day in `~/.viv/acp-registry.json`), with a built-in list for when it can't be fetched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use vivianne_ai::paths::{viv_home_path, write_json_atomic};

use super::install::receipt_for;

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
    /// The launcher (`npx` or `uvx`, or `node` for an installed agent) is on this computer.
    pub available: bool,
    /// Its package has been fetched and started here at least once, so a turn won't wait on a download.
    pub ready: bool,
    /// What starts it: `npx`, `uvx`, or `node` when it is installed under `~/.viv/agents`.
    pub launcher: String,
    pub(super) args: Vec<String>,
    pub(super) env: HashMap<String, String>,
    /// The npm package (`name@version`) an `npx` agent comes from.
    pub(super) package: Option<String>,
    /// The package's own extra arguments (`--acp`, ...).
    pub(super) extra: Vec<String>,
    /// Installed into `~/.viv/agents`: it starts as `node <script>`, with no `npx` and no network.
    pub managed: bool,
}

impl Harness {
    /// Whether its package comes from npm, and so has to be set up before its first turn.
    #[must_use]
    pub fn needs_setup(&self) -> bool {
        self.package.is_some() && !self.managed && !self.ready
    }

    /// The package to install, for an agent that comes from npm.
    #[must_use]
    pub fn package(&self) -> Option<&str> {
        self.package.as_deref()
    }

    /// The launcher's arguments, for showing a command the user can run.
    pub(super) fn args_for_display(&self) -> Vec<String> {
        self.args.clone()
    }

    /// What the launcher is called to the user, for "X isn't installed".
    #[must_use]
    pub fn launcher_name(&self) -> &str {
        if self.managed { "node" } else { &self.launcher }
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

fn on_path(program: &str) -> bool {
    which::which(program).is_ok()
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
    let extra: Vec<String> = extra.and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
    let mut args = if launcher == "npx" { vec!["-y".to_owned(), package.to_owned()] } else { vec![package.to_owned()] };
    args.extend(extra.iter().cloned());
    let env = env
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))).collect())
        .unwrap_or_default();
    let text = |key: &str| agent.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Harness {
        name: text("name").unwrap_or_else(|| id.clone()),
        description: text("description").unwrap_or_default(),
        icon: text("icon"),
        available: on_path(launcher),
        ready: false,
        launcher: launcher.to_owned(),
        args,
        env,
        package: (launcher == "npx").then(|| package.to_owned()),
        extra,
        managed: false,
        id,
    })
}

fn fallback() -> Vec<Harness> {
    let npx = on_path("npx");
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
            package: Some((*package).to_owned()),
            extra: extra.iter().map(|s| (*s).to_owned()).collect(),
            managed: false,
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
    let node = on_path("node");
    for harness in &mut list {
        harness.ready = ready.contains(&harness.id);
        // An agent installed under ~/.viv/agents starts as `node <script>`.
        if let Some(receipt) = harness.package.as_deref().and_then(|package| receipt_for(&harness.id, package))
            && node
        {
            "node".clone_into(&mut harness.launcher);
            harness.args = std::iter::once(receipt.script).chain(harness.extra.iter().cloned()).collect();
            harness.available = true;
            harness.ready = true;
            harness.managed = true;
        }
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

/// Whether the agent with this id can run now: it is in the list and its launcher is on this computer.
#[must_use]
pub fn is_usable(list: &[Harness], id: &str) -> bool {
    list.iter().any(|h| h.id == id && h.available)
}

// ── npx's cache ─────────────────────────────────────────────────────────────

/// npx's cache folder: `npm_config_cache`, else the platform's usual place.
fn npx_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("npm_config_cache") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("npm-cache"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".npm"))
    }
}

/// The script an npx package's bin points at, when npx has already fetched that exact `name@version` into its
/// cache. Started as `node <script>` it skips npx's own launch, which on Windows runs the bin through a cmd
/// shell that could fail to find it ("'claude-agent-acp' is not recognized") even though the package was in
/// the cache.
pub(super) fn npx_cached_script(package: &str) -> Option<PathBuf> {
    npx_cached_script_in(&npx_cache_dir()?, package)
}

fn npx_cached_script_in(cache: &Path, package: &str) -> Option<PathBuf> {
    let name = match package.rfind('@') {
        Some(i) if i > 0 => &package[..i],
        _ => package,
    };
    for entry in std::fs::read_dir(cache.join("_npx")).ok()?.flatten() {
        let dir = entry.path();
        let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else { continue };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else { continue };
        let wanted = manifest
            .pointer("/_npx/packages")
            .and_then(Value::as_array)
            .is_some_and(|packages| packages.iter().any(|v| v.as_str() == Some(package)));
        if !wanted {
            continue;
        }
        let package_dir = dir.join("node_modules").join(name);
        let Ok(text) = std::fs::read_to_string(package_dir.join("package.json")) else { continue };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else { continue };
        let Some(bin) = super::install::bin_script(&manifest, name) else { continue };
        let script = package_dir.join(bin);
        if script.is_file() {
            return Some(script);
        }
    }
    None
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
        assert_eq!(list[0].package(), Some("@google/gemini-cli@0.66.0"));
        assert_eq!((list[1].launcher.as_str(), list[1].args.clone(), list[1].env["A"].as_str()), ("uvx", vec!["tool==1.0".into()], "b"));
        assert_eq!(list[1].package(), None);
    }

    #[test]
    fn the_built_in_list_runs_through_npx_and_needs_setup() {
        let list = fallback();
        assert!(list.iter().all(|h| h.launcher == "npx" && h.args[0] == "-y"));
        assert!(list.iter().all(Harness::needs_setup));
        assert!(list.iter().any(|h| h.id == "claude-acp"));
    }

    #[test]
    fn the_cached_script_is_found_for_the_exact_version_only() {
        let cache = std::env::temp_dir().join(format!("viv-npx-{}", std::process::id()));
        let entry = cache.join("_npx").join("abc");
        let package = entry.join("node_modules").join("@scope").join("agent");
        std::fs::create_dir_all(package.join("dist")).unwrap_or_default();
        std::fs::write(entry.join("package.json"), r#"{"_npx":{"packages":["@scope/agent@1.2.3"]}}"#).unwrap_or_default();
        std::fs::write(package.join("package.json"), r#"{"bin":{"agent":"dist/index.js"}}"#).unwrap_or_default();
        std::fs::write(package.join("dist").join("index.js"), "").unwrap_or_default();
        assert_eq!(npx_cached_script_in(&cache, "@scope/agent@1.2.3"), Some(package.join("dist/index.js")));
        assert_eq!(npx_cached_script_in(&cache, "@scope/agent@9.9.9"), None);
        assert_eq!(npx_cached_script_in(&cache.join("missing"), "@scope/agent@1.2.3"), None);
        let _ = std::fs::remove_dir_all(cache);
    }

    #[test]
    fn only_an_agent_on_this_computer_is_usable() {
        let mut list = fallback();
        list[0].available = true;
        list[1].available = false;
        assert!(is_usable(&list, "claude-acp"));
        assert!(!is_usable(&list, "codex-acp"));
        assert!(!is_usable(&list, "nope"));
    }
}
