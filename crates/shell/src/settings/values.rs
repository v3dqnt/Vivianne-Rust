//! The values the Settings pages edit. Held in memory only: nothing is saved
//! or applied yet. Saving into `~/.viv` (the format the old app wrote) comes
//! with the settings store. Demo lists (models, apps, catalog entries) are
//! generated in `demo` so this file stays types and defaults.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default)]
pub struct Values {
    // Account & usage — filled by `demo::fill`.
    pub usage_days: Vec<UsageDay>,
    pub model_usage: Vec<ModelUsage>,
    /// Heatmap view: 0 daily, 1 weekly, 2 cumulative.
    pub usage_mode: u8,
    /// Usage-by-model range: "7", "30" or "all".
    pub usage_range: String,

    // Providers.
    pub open_provider: String,
    pub api_keys: BTreeMap<String, String>,
    pub pool: BTreeMap<String, Vec<PoolKey>>,
    pub rotation: BTreeMap<String, String>,
    pub chatgpt_connected: bool,
    pub chatgpt_email: String,
    pub chatgpt_shared_at_signin: bool,
    pub routing_enabled: bool,
    pub quick_model: String,
    pub tier_easy: String,
    pub tier_medium: String,
    pub tier_hard: String,
    pub routing_probe: Option<String>,
    pub agents: Vec<CodingAgent>,
    pub agent_trace: Option<Vec<String>>,
    pub agent_log: String,
    pub agents_show_all: bool,
    pub custom_providers: Vec<CustomProvider>,
    pub open_custom_provider: Option<usize>,
    /// Draft text of every transient input, keyed by element id
    /// (`ckey:{provider}`, `cname:{id}`, pool drafts, the routing probe…).
    pub key_drafts: BTreeMap<String, String>,
    /// Whether each provider's key-pool panel is expanded.
    pub pool_open: BTreeMap<String, bool>,
    /// Staged state while a brand-new custom provider is built up: the
    /// provider object holds the format and its models before the first save.
    pub custom_draft: Option<CustomProvider>,
    /// "Match the model to the task" — only meaningful with routing on.
    pub routing_auto: bool,
    /// Whether the add-model dialog's Advanced section is open.
    pub add_model_advanced: bool,

    // Local models.
    pub local_tab: usize,
    pub endpoints: Vec<LocalEndpoint>,
    pub hardware: Option<Hardware>,
    pub llama_ready: bool,
    pub llama_progress: Option<u32>,
    pub hf_query: String,
    pub hf_results: Vec<HfRepo>,
    pub pulls: BTreeMap<String, u32>,
    pub pulled: Vec<String>,
    pub ollama_models: Vec<String>,
    pub ollama_used: Vec<String>,
    pub ollama_pull_input: String,
    pub ollama_pull: Option<(String, u32)>,
    pub sd_ready: bool,
    pub sd_progress: Option<u32>,
    pub sd_path: Option<String>,
    pub image_models: Vec<ImageModel>,

    // Reliability.
    pub retry_transient: bool,
    pub fallbacks: Vec<FallbackModel>,
    pub limits: Vec<Limit>,

    // Mods.
    pub mods: Vec<ModEntry>,
    pub guides_repo: String,
    pub tools_open: bool,
    pub tools: Vec<ToolEntry>,
    pub custom_tools: Vec<UsedTool>,
    pub shredder_note: Option<String>,

    // Connections.
    pub apps: Vec<AppEntry>,
    pub conn_search: String,
    pub conn_cat: usize,
    pub mcp: Vec<McpServer>,

    // Web & browser.
    pub search_provider: String,
    pub search_active: String,
    pub search_test: Option<String>,
    pub web_keys: BTreeMap<String, String>,
    pub browser_connected: bool,
    pub browsers: Vec<Browser>,

    // Memory.
    pub remember_across_sessions: bool,
    pub auto_consolidate: bool,
    pub auto_extract: bool,
    pub refuse_secrets: bool,
    pub memory_entries: u32,
    pub memory_tokens: u32,
    pub memory_mb: f32,
    pub memory_pages: Vec<MemoryPage>,
    pub pages_dirty: bool,
    pub hindsight_on: bool,
    pub hs_address: String,
    pub hs_bank: String,
    pub hs_key: String,
    pub hs_send: bool,
    pub hs_test: Option<String>,
    pub local_embedder: String,
    pub embedder_progress: Option<(String, u32)>,
    pub cloud_prefer: String,
    pub cloud_provider: String,
    pub cloud_model: String,
    pub cloud_test: Option<String>,
    pub mem_query: String,
    pub mem_results: Vec<MemoryHit>,
    pub tidy_note: Option<String>,

    // Vault.
    pub vault: Vec<VaultEntry>,

    // Skills.
    pub skills: Vec<Skill>,
    pub skill_search: String,
    pub hub_open: bool,
    pub hub_source: String,
    pub hub_query: String,
    pub hub_shown: usize,
    pub hub: Vec<HubSkill>,

    // Diagnostics.
    pub share_diagnostics: bool,
    pub share_endpoint: String,
    pub diag_events: u32,
    pub diag_since: String,
    pub diag_kinds: Vec<(String, u32)>,
    pub crashes: Vec<Crash>,
    pub open_crash: Option<usize>,
    pub report_text: String,
    pub report_attach: bool,
    pub report_result: Option<String>,

    // Appearance.
    pub mode: String,
    pub scale: f32,
    pub chat_density: String,
    pub chat_width: String,
    pub visuals: String,
    pub read_aloud: bool,
    pub robot_scale: f32,
    pub robot_move: String,
    pub face: usize,

    // Layout & shortcuts.
    pub sidebar_side: String,
    pub shortcuts: Vec<Shortcut>,
    pub recording: Option<usize>,
    pub rec_warning: Option<String>,

    // Components.
    pub components: Vec<ComponentEntry>,

    // Updates.
    pub update_status: UpdateStatus,
    pub install_progress: Option<f32>,
    pub releases: Vec<Release>,
}

/// One day of usage, counted from today back (`offset` days ago).
#[derive(Debug, Clone)]
pub struct UsageDay {
    pub offset: u32,
    pub requests: u32,
    pub input: u64,
    pub output: u64,
    pub cost: Option<f64>,
}

/// Tokens and spend attributed to one model over the selected range.
#[derive(Debug, Clone)]
pub struct ModelUsage {
    pub model: String,
    pub provider: String,
    pub requests: u32,
    pub input: u64,
    pub output: u64,
    pub cost: Option<f64>,
}

/// A key registered in a provider's pool.
#[derive(Debug, Clone)]
pub struct PoolKey {
    pub label: String,
    pub key: String,
    pub requests: u32,
    /// 0 ready, 1 quota/billing, 2 rate limited, 3 failed.
    pub state: u8,
}

pub const KEY_READY: u8 = 0;
pub const KEY_QUOTA: u8 = 1;
pub const KEY_RATE: u8 = 2;
pub const KEY_FAIL: u8 = 3;

/// A coding agent the Agent Client Protocol can run.
#[derive(Debug, Clone)]
pub struct CodingAgent {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub launcher: String,
    /// 0 ready, 1 needs setup, 2 not installed.
    pub state: u8,
    /// Downloaded bytes, total bytes, while `Set up` runs.
    pub downloading: Option<(u64, u64)>,
}

pub const AGENT_READY: u8 = 0;
pub const AGENT_SETUP: u8 = 1;
pub const AGENT_MISSING: u8 = 2;

/// A user-defined provider: base URL, API format and its models.
#[derive(Debug, Clone)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub format: String,
    pub has_key: bool,
    pub models: Vec<CustomModel>,
}

#[derive(Debug, Clone)]
pub struct CustomModel {
    pub id: String,
    pub ctx: u64,
    pub max_out: Option<u64>,
}

/// A model served by llama.cpp or mistral.rs on this machine.
#[derive(Debug, Clone)]
pub struct LocalEndpoint {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub file: String,
    pub port: u32,
    pub ctx: u32,
    pub ngl: u32,
    pub kv: String,
    pub device: String,
    pub threads: String,
    pub reuse: String,
    pub speculative: String,
    pub cache_ram: String,
    pub idle: String,
    pub mlock: bool,
    pub extra: String,
    /// 0 stopped, 1 running, 2 starting, 3 crashed.
    pub state: u8,
    pub more_open: bool,
    pub more: BTreeMap<String, String>,
}

pub const EP_STOPPED: u8 = 0;
pub const EP_RUNNING: u8 = 1;
pub const EP_STARTING: u8 = 2;
pub const EP_CRASHED: u8 = 3;

/// What this machine offers a local model.
#[derive(Debug, Clone)]
pub struct Hardware {
    pub ram_gb: u32,
    pub threads: u32,
    pub gpus: Vec<(String, u32)>,
    pub unified: bool,
    pub os: String,
}

/// A Hugging Face repo found by the Get-models search.
#[derive(Debug, Clone)]
pub struct HfRepo {
    pub repo: String,
    pub downloads: u64,
    pub files: Vec<(String, f64)>,
}

#[derive(Debug, Clone)]
pub struct ImageModel {
    pub id: String,
    pub name: String,
    pub gb: f64,
    pub note: String,
    pub owned: bool,
}

/// A fallback model, tried in order after the selected one.
#[derive(Debug, Clone)]
pub struct FallbackModel {
    pub id: String,
    pub label: String,
}

/// A staged limit with a draft value and the built-in default.
#[derive(Debug, Clone)]
pub struct Limit {
    pub id: String,
    pub label: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub hint: String,
}

#[derive(Debug, Clone)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub enabled: bool,
}

/// A built-in tool the agent can see, grouped by capability.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub group: String,
}

/// A tool Vivianne made during real use.
#[derive(Debug, Clone)]
pub struct UsedTool {
    pub name: String,
    pub uses: u32,
    pub last: String,
}

/// An app the agent can connect to.
#[derive(Debug, Clone)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    /// 0 sign in with your account, 1 no sign-in needed, 2 token, 3 bot token.
    pub auth: u8,
    pub account: Option<String>,
    pub connected: bool,
    pub pending: bool,
    pub desc: String,
}

pub const AUTH_OAUTH: u8 = 0;
pub const AUTH_OPEN: u8 = 1;
pub const AUTH_TOKEN: u8 = 2;
pub const AUTH_BOT: u8 = 3;

/// An MCP server the agent's tools come from.
#[derive(Debug, Clone)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub detail: String,
    pub tools: u32,
    pub env: String,
}

/// A browser on this machine.
#[derive(Debug, Clone)]
pub struct Browser {
    pub name: String,
    /// 0 connected, 1 installed and open but no extension, 2 not installed,
    /// 3 installed but not open.
    pub state: u8,
}

pub const BROWSER_CONNECTED: u8 = 0;
pub const BROWSER_NO_EXT: u8 = 1;
pub const BROWSER_NOT_INSTALLED: u8 = 2;
pub const BROWSER_NOT_OPEN: u8 = 3;

#[derive(Debug, Clone)]
pub struct MemoryPage {
    pub name: String,
    pub question: String,
}

#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub text: String,
    pub date: String,
}

/// A saved secret. The value itself is never kept here.
#[derive(Debug, Clone)]
pub struct VaultEntry {
    pub id: String,
    pub label: String,
    /// 0 login, 1 card, 2 secret.
    pub kind: u8,
    pub site: String,
    pub username: String,
    pub hint: String,
    pub used: String,
}

pub const VAULT_LOGIN: u8 = 0;
pub const VAULT_CARD: u8 = 1;
pub const VAULT_SECRET: u8 = 2;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub desc: String,
    pub path: String,
}

/// One entry of a skills catalog.
#[derive(Debug, Clone)]
pub struct HubSkill {
    pub name: String,
    pub dir: String,
    pub desc: String,
    pub files: u32,
    pub installed: bool,
    pub modified: bool,
}

#[derive(Debug, Clone)]
pub struct Crash {
    pub time: String,
    pub kind: String,
    pub trace: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Shortcut {
    pub id: String,
    pub label: String,
    pub group: String,
    pub default_keys: String,
    pub keys: String,
}

#[derive(Debug, Clone)]
pub struct ComponentEntry {
    pub id: String,
    pub name: String,
    pub size: String,
    pub desc: String,
    pub installed: bool,
    pub running: bool,
    pub progress: Option<f32>,
    pub note: Option<String>,
}

/// Where the Updates page stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Available(String),
    Latest,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub date: String,
    pub notes: Vec<String>,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self::Idle
    }
}

impl Values {
    /// Shell command timeout from the limits list, for whoever wires the store.
    pub fn shell_timeout_secs(&self) -> f64 {
        self.limits
            .iter()
            .find(|l| l.id == "shellTimeoutSecs")
            .map_or(120.0, |l| l.value)
    }
}

/// The values, shared between the page builders and the field callbacks.
pub type Shared = Rc<RefCell<Values>>;
