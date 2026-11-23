//! Providers the user defines: any server that speaks one of the four wire formats, with a base URL, a key
//! and a hand-entered list of models. Kept in `~/.viv/agent/settings.json` under `customProviders` (the
//! old app's place); the key is not part of that: it goes where every provider's key goes, under this
//! provider's id.
//!
//! The id is `custom:<format>:<uuid>`, because `provider_registry::provider_spec` has nothing but the id to
//! tell which wire format a custom provider speaks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host::{read_settings_json, update_settings};

const KEY: &str = "customProviders";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiFormat {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GoogleGenerative,
}

impl ApiFormat {
    pub const ALL: [Self; 4] = [
        Self::AnthropicMessages,
        Self::OpenAiChat,
        Self::OpenAiResponses,
        Self::GoogleGenerative,
    ];

    /// The name in the provider id and in settings.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropicMessages",
            Self::OpenAiChat => "openAiChat",
            Self::OpenAiResponses => "openAiResponses",
            Self::GoogleGenerative => "googleGenerative",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "Anthropic messages (/v1/messages)",
            Self::OpenAiChat => "OpenAI chat completions (/v1/chat/completions)",
            Self::OpenAiResponses => "OpenAI responses (/v1/responses)",
            Self::GoogleGenerative => "Google generative AI (/v1beta)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModel {
    pub id: String,
    pub context_window: u64,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_format: ApiFormat,
    #[serde(default)]
    pub models: Vec<CustomModel>,
}

/// A new provider id for `format`.
#[must_use]
pub fn new_id(format: ApiFormat) -> String {
    format!("custom:{}:{}", format.name(), uuid::Uuid::new_v4())
}

/// The providers in settings. Entries that do not parse are skipped.
pub async fn list() -> Vec<CustomProvider> {
    from_settings(&read_settings_json().await)
}

fn from_settings(settings: &Value) -> Vec<CustomProvider> {
    settings
        .get(KEY)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
        .collect()
}

/// Adds the provider, or replaces the one with the same id.
pub async fn save(provider: CustomProvider) -> Result<(), String> {
    update_settings(move |settings| {
        let mut all = from_settings(settings);
        match all.iter_mut().find(|p| p.id == provider.id) {
            Some(existing) => *existing = provider,
            None => all.push(provider),
        }
        put(settings, &all);
    })
    .await
}

pub async fn remove(id: &str) -> Result<(), String> {
    let id = id.to_owned();
    update_settings(move |settings| {
        let mut all = from_settings(settings);
        all.retain(|p| p.id != id);
        put(settings, &all);
    })
    .await
}

fn put(settings: &mut Value, providers: &[CustomProvider]) {
    if let Some(object) = settings.as_object_mut() {
        object.insert(
            KEY.to_owned(),
            serde_json::to_value(providers).unwrap_or(Value::Null),
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_new_id_carries_its_format_so_the_registry_can_read_it() {
        for format in ApiFormat::ALL {
            let id = new_id(format);
            assert!(
                crate::ai::provider_registry::provider_spec(&id).is_some(),
                "{id}"
            );
        }
    }

    #[test]
    fn the_old_apps_entries_are_read_and_broken_ones_skipped() {
        let settings = json!({"customProviders": [
            {"id": "custom:openAiChat:1", "name": "Acme", "baseUrl": "https://x/v1", "apiFormat": "openAiChat",
             "models": [{"id": "m", "contextWindow": 1000, "maxOutputTokens": null}]},
            {"id": "broken"}
        ]});
        let all = from_settings(&settings);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].models[0].context_window, 1000);
        assert_eq!(all[0].api_format, ApiFormat::OpenAiChat);
    }

    #[test]
    fn other_settings_survive_a_write() {
        let mut settings = json!({"theme": "dark"});
        put(&mut settings, &[]);
        assert_eq!(settings["theme"], "dark");
        assert!(settings["customProviders"].is_array());
    }
}
