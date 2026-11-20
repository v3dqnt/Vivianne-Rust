//! MCP — a Model Context Protocol client, implemented directly against the
//! public spec (JSON-RPC 2.0; stdio transport is newline-delimited JSON,
//! distinct from LSP's Content-Length framing; HTTP transport POSTs
//! JSON-RPC and reads a JSON response). Supports local stdio servers and
//! remote HTTP servers, including OAuth 2.1 + PKCE for the latter.

// ponytail: the old app's MCP client, ported as it was; these are style lints it was not written against.
#![allow(clippy::collapsible_if, clippy::large_enum_variant)]

pub mod oauth;
pub mod transport;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use transport::McpConnection;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransportConfig,
    /// Request/response timeout in milliseconds — applies uniformly
    /// regardless of transport kind (`McpConnection::connect` reads this
    /// once and uses it for both the HTTP client's per-request timeout and
    /// the stdio JSON-RPC response wait). `None` keeps the prior hardcoded
    /// default of 30s.
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    /// The name whose saved OAuth sign-in (`oauth`) each request to a remote server carries. The token is kept in
    /// `~/.viv/mcp_oauth.json`, never in the config file.
    #[serde(default)]
    pub auth: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_schema", rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

fn default_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

/// `~/.viv/<parts>`.
pub(crate) fn viv_home_path(parts: &[&str]) -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    parts.iter().fold(home.join(".viv"), |p, part| p.join(part))
}

/// Bumped whenever a server says its tool list changed, so whoever caches the tools knows to read them again.
static TOOLS_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn tools_changed() {
    TOOLS_VERSION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Changes each time a server's tool list changed.
pub fn tools_version() -> u64 {
    TOOLS_VERSION.load(std::sync::atomic::Ordering::SeqCst)
}

lazy_static::lazy_static! {
    static ref CONNECTIONS: RwLock<HashMap<String, Arc<McpConnection>>> = RwLock::new(HashMap::new());
    static ref CONFIGS: RwLock<HashMap<String, McpServerConfig>> = RwLock::new(HashMap::new());
    /// The connected servers' names, readable without awaiting, so a tool call can be routed from its public name.
    static ref SERVER_NAMES: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());
    /// Each server's last tool list that was read successfully: a failed re-list keeps these instead of the tools vanishing.
    static ref LAST_TOOLS: RwLock<HashMap<String, Vec<McpToolDef>>> = RwLock::new(HashMap::new());
}

/// The server and the tool a public name `mcp__<server>__<tool>` stands for, given the servers there are. The longest server name
/// that fits wins, so a server called `foo__bar` gets `mcp__foo__bar__baz` rather than a server `foo` being asked for `bar__baz`.
pub fn split_public_name<'a>(name: &str, servers: impl IntoIterator<Item = &'a str>) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    servers
        .into_iter()
        .filter(|s| rest.len() > s.len() + 2 && rest.starts_with(*s) && rest[s.len()..].starts_with("__"))
        .max_by_key(|s| s.len())
        .map(|s| (s.to_string(), rest[s.len() + 2..].to_string()))
}

/// Route a tool call by its public name to a connected server.
pub fn route(name: &str) -> Option<(String, String)> {
    let names = SERVER_NAMES.read().unwrap_or_else(|e| e.into_inner());
    split_public_name(name, names.iter().map(String::as_str))
}

fn remember_names(names: Vec<String>) {
    *SERVER_NAMES.write().unwrap_or_else(|e| e.into_inner()) = names;
}

/// `~/.viv/agent/mcp_servers.json` — every connected server's config, so it
/// can be reconnected on the next launch via `restore_servers`. Plaintext,
/// same trust boundary as `settings.json` — NOT an encrypted vault. A stdio
/// server's `env` map can carry a secret (e.g. `GITHUB_TOKEN`); this is a
/// known, accepted limitation (matching this app's credentials.json
/// plaintext-fallback precedent) rather than a half-built secrets store.
fn configs_path() -> Option<std::path::PathBuf> {
    viv_home_path(&["agent"]).join("mcp_servers.json").into()
}

async fn persist_configs() {
    let Some(path) = configs_path() else { return };
    let configs: Vec<McpServerConfig> = CONFIGS.read().await.values().cloned().collect();
    let Ok(json) = serde_json::to_string_pretty(&configs) else { return };
    if let Err(e) = tokio::fs::write(&path, json).await {
        log::warn!("mcp: failed to persist server configs: {e}");
    }
}

/// Read the persisted server list from disk (empty if the file doesn't
/// exist yet or fails to parse) — pass the result to `restore_servers` at
/// startup to reconnect everything from the previous session.
pub async fn load_persisted_configs() -> Vec<McpServerConfig> {
    let Some(path) = configs_path() else { return Vec::new() };
    let Ok(content) = tokio::fs::read_to_string(&path).await else { return Vec::new() };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Connect to a server, perform the `initialize` handshake, and cache its
/// tool list. Persists to disk on success so it survives an app restart.
pub async fn add_server(config: McpServerConfig) -> Result<Vec<McpToolDef>, String> {
    // A server added again under its name replaces the earlier connection.
    remove_connection(&config.name).await;
    let connection = McpConnection::connect(&config).await?;
    let tools = connection.list_tools().await?;
    CONNECTIONS.write().await.insert(config.name.clone(), Arc::new(connection));
    LAST_TOOLS.write().await.insert(config.name.clone(), tools.clone());
    CONFIGS.write().await.insert(config.name.clone(), config);
    remember_names(CONFIGS.read().await.keys().cloned().collect());
    persist_configs().await;
    tools_changed();
    Ok(tools)
}

async fn remove_connection(name: &str) {
    let old = CONNECTIONS.write().await.remove(name);
    if let Some(conn) = old {
        conn.close().await;
    }
}

pub async fn remove_server(name: &str) -> bool {
    let removed_conn = CONNECTIONS.write().await.remove(name);
    if let Some(conn) = removed_conn {
        conn.close().await;
    }
    let removed = CONFIGS.write().await.remove(name).is_some();
    LAST_TOOLS.write().await.remove(name);
    remember_names(CONFIGS.read().await.keys().cloned().collect());
    if removed {
        persist_configs().await;
        tools_changed();
    }
    removed
}

pub async fn list_servers() -> Vec<McpServerConfig> {
    CONFIGS.read().await.values().cloned().collect()
}

/// All tools across every connected server, namespaced as `mcp__<server>__<tool>`
/// so they can be routed back to the right connection when called.
pub async fn list_all_tools() -> Vec<(String, McpToolDef)> {
    let mut out = Vec::new();
    let connections = CONNECTIONS.read().await;
    for (server, conn) in connections.iter() {
        // A failed re-list keeps the server's last good tools: a network blip shouldn't change the model's tools mid-conversation.
        let tools = match conn.list_tools().await {
            Ok(tools) => {
                LAST_TOOLS.write().await.insert(server.clone(), tools.clone());
                tools
            }
            Err(e) => {
                log::warn!("mcp: couldn't re-list '{server}' ({e}); keeping its last tool list");
                LAST_TOOLS.read().await.get(server).cloned().unwrap_or_default()
            }
        };
        for tool in tools {
            out.push((server.clone(), tool));
        }
    }
    out
}

/// The tools as the model sees them, readable without awaiting: `(public name, description, parameters)`. Filled by
/// [`refresh_tools`]; empty until a server has connected.
static SNAPSHOT: std::sync::RwLock<Vec<(String, String, serde_json::Value)>> = std::sync::RwLock::new(Vec::new());
static SNAPSHOT_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

#[must_use]
pub fn definitions() -> Vec<(String, String, serde_json::Value)> {
    SNAPSHOT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Reads every server's tools again if any server's list changed since the last time. Cheap when nothing did.
pub async fn refresh_tools() {
    let version = tools_version();
    if SNAPSHOT_VERSION.swap(version, std::sync::atomic::Ordering::SeqCst) == version {
        return;
    }
    let mut tools: Vec<(String, String, serde_json::Value)> = list_all_tools()
        .await
        .into_iter()
        .map(|(server, tool)| (format!("mcp__{server}__{}", tool.name), tool.description, tool.input_schema))
        .collect();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    *SNAPSHOT.write().unwrap_or_else(|e| e.into_inner()) = tools;
}

/// Runs a tool by its public name (`mcp__<server>__<tool>`).
pub async fn call_public(name: &str, args: serde_json::Value) -> Result<String, String> {
    let (server, tool) = route(name).ok_or_else(|| format!("no connected MCP server has a tool called {name}"))?;
    call_tool(&server, &tool, args).await
}

pub async fn call_tool(server_name: &str, tool_name: &str, args: serde_json::Value) -> Result<String, String> {
    let conn = {
        let connections = CONNECTIONS.read().await;
        connections.get(server_name).cloned()
    };
    let Some(conn) = conn else { return Err(format!("MCP server '{server_name}' is not connected")) };
    conn.call_tool(tool_name, args).await
}

/// Reconnect every server persisted from a previous run (called at startup).
pub async fn restore_servers(configs: Vec<McpServerConfig>) {
    for config in configs {
        let name = config.name.clone();
        if let Err(e) = add_server(config).await {
            log::warn!("mcp: failed to reconnect server '{name}': {e}");
        }
    }
}

pub async fn shutdown_all() {
    let mut connections = CONNECTIONS.write().await;
    for (_, conn) in connections.drain() {
        conn.close().await;
    }
}

#[cfg(test)]
mod name_tests {
    use super::split_public_name;

    #[test]
    fn a_public_name_routes_to_the_longest_server_that_fits() {
        let servers = ["foo", "foo__bar", "linear"];
        assert_eq!(split_public_name("mcp__foo__bar__baz", servers), Some(("foo__bar".into(), "baz".into())));
        assert_eq!(split_public_name("mcp__foo__qux", servers), Some(("foo".into(), "qux".into())));
        assert_eq!(split_public_name("mcp__linear__create_issue", servers), Some(("linear".into(), "create_issue".into())));
        assert_eq!(split_public_name("mcp__unknown__x", servers), None);
        assert_eq!(split_public_name("mcp__linear__", servers), None);
        assert_eq!(split_public_name("read", servers), None);
    }
}
