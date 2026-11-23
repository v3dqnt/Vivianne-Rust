//! What the Providers page shows and edits. Read from where the keys and settings live (see
//! `vivianne_ai::providers`), never invented. The page keeps this in `Values` so every part of it
//! redraws from one place.

use std::collections::BTreeMap;

use vivianne_ai::ai::auth::{PoolEntryInfo, RotationStrategy};
use vivianne_ai::custom_providers::{ApiFormat, CustomProvider};
use vivianne_ai::inference::difficulty::Verdict;
use vivianne_ai::inference::routing::Config as RoutingConfig;
use vivianne_ai::providers::KeySource;

/// The providers that take one API key, as (id, name): the ids of the old provider registry.
pub const CLOUD_PROVIDERS: [(&str, &str); 15] = [
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("google", "Google"),
    ("groq", "Groq"),
    ("xai", "xAI"),
    ("deepseek", "DeepSeek"),
    ("mistral", "Mistral"),
    ("together", "Together AI"),
    ("fireworks", "Fireworks AI"),
    ("openrouter", "OpenRouter"),
    ("cerebras", "Cerebras"),
    ("nvidia-nim", "NVIDIA NIM"),
    ("nebius", "Nebius Token Factory"),
    ("huggingface", "Hugging Face"),
    ("cloudflare-workers-ai", "Cloudflare Workers AI"),
];

/// What is open on the right of the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Nothing,
    Cloud(String),
    Custom(String),
    NewCustom,
}

/// A model the routing pickers can choose from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickableModel {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub api: String,
}

impl PickableModel {
    /// The value a dropdown holds for this model.
    #[must_use]
    pub fn token(&self) -> String {
        token(&self.provider, &self.id)
    }
}

#[must_use]
pub fn token(provider: &str, id: &str) -> String {
    format!("{provider}/{id}")
}

/// A coding agent from the Agent Client Protocol registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub launcher: String,
    /// The launcher is on this computer.
    pub available: bool,
    /// It has been downloaded and started here before.
    pub ready: bool,
}

/// Whether a coding agent is signed in, as far as it has been asked.
#[derive(Debug, Clone)]
pub enum AgentAuth {
    Checking,
    SignedIn,
    /// It has to be signed in first, one of these ways.
    Needs(Vec<vivianne_providers::acp::AuthMethod>),
    Failed(String),
}

/// What testing an agent found.
#[derive(Debug, Clone, Default)]
pub struct AgentReport {
    /// (step, worked, seconds, detail)
    pub steps: Vec<(String, bool, f64, String)>,
    pub reply: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderState {
    pub pane: Pane,
    /// Set once the first read of keys and settings is back.
    pub loaded: bool,
    pub sources: BTreeMap<String, Option<KeySource>>,
    /// Keys of the open provider (main first) and how they are taken in turn.
    pub pool: Vec<PoolEntryInfo>,
    pub strategy: RotationStrategy,
    pub custom: Vec<CustomProvider>,
    /// Custom providers that have a key saved.
    pub custom_keyed: Vec<String>,
    /// The form for a new custom provider, and models added to it before it is first saved.
    pub new_format: Option<ApiFormat>,
    pub new_models: Vec<(String, u64)>,
    pub agents: Option<Vec<AgentInfo>>,
    pub agent_reports: BTreeMap<String, AgentReport>,
    pub agent_auth: BTreeMap<String, AgentAuth>,
    /// What setting up an agent is doing now (the last thing npm did, and how much is on disk).
    pub agent_progress: BTreeMap<String, String>,
    pub agent_log: Option<Vec<String>>,
    pub routing: Option<RoutingConfig>,
    pub models: Vec<PickableModel>,
    pub verdict: Option<Verdict>,
    /// Text fields, by name.
    pub drafts: BTreeMap<String, String>,
    /// Something is being done (saving, signing in...), by name, so its button can say so.
    pub busy: Vec<String>,
    /// The last failure, to show where the user is looking.
    pub error: Option<String>,
}

impl ProviderState {
    #[must_use]
    pub fn draft(&self, name: &str) -> String {
        self.drafts.get(name).cloned().unwrap_or_default()
    }

    pub fn set_draft(&mut self, name: &str, text: String) {
        self.drafts.insert(name.to_owned(), text);
    }

    pub fn clear_draft(&mut self, name: &str) {
        self.drafts.remove(name);
    }

    #[must_use]
    pub fn is_busy(&self, what: &str) -> bool {
        self.busy.iter().any(|b| b == what)
    }

    pub fn start(&mut self, what: &str) {
        if !self.is_busy(what) {
            self.busy.push(what.to_owned());
        }
        self.error = None;
    }

    pub fn finish(&mut self, what: &str) {
        self.busy.retain(|b| b != what);
    }

    /// Whether the provider has a key, in Vivianne or in the environment.
    #[must_use]
    pub fn connected(&self, provider: &str) -> bool {
        matches!(
            self.sources.get(provider),
            Some(Some(KeySource::Saved | KeySource::Environment))
        )
    }
}

/// The name shown for a provider id.
#[must_use]
pub fn provider_name(id: &str) -> String {
    CLOUD_PROVIDERS
        .iter()
        .find(|(p, _)| *p == id)
        .map_or_else(|| id.to_owned(), |(_, name)| (*name).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_matches_the_old_apps_provider_ids() {
        for (id, _) in CLOUD_PROVIDERS {
            assert!(
                vivianne_ai::ai::provider_registry::provider_spec(id).is_some(),
                "{id} is not a known provider"
            );
        }
    }

    #[test]
    fn busy_names_come_and_go_and_starting_clears_the_last_error() {
        let mut state = ProviderState {
            error: Some("x".into()),
            ..ProviderState::default()
        };
        state.start("save");
        state.start("save");
        assert!(state.is_busy("save") && state.busy.len() == 1 && state.error.is_none());
        state.finish("save");
        assert!(!state.is_busy("save"));
    }

    #[test]
    fn a_provider_is_connected_by_a_saved_or_environment_key_only() {
        let mut state = ProviderState::default();
        state.sources.insert("a".into(), Some(KeySource::Saved));
        state
            .sources
            .insert("b".into(), Some(KeySource::Environment));
        state.sources.insert("c".into(), Some(KeySource::None));
        assert!(state.connected("a") && state.connected("b"));
        assert!(!state.connected("c") && !state.connected("d"));
    }
}
