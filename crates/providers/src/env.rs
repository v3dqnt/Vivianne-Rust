//! Pick a provider and model: from the old Vivianne's saved choices and keys, or from environment
//! variables when set. Temporary: the Settings page replaces it.
//!
//! Keys, first found wins: the ones the old app saved (the OS keychain, then `~/.viv/agent/auth.json`),
//! then the usual variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`...;
//! `AI_GATEWAY_API_KEY` for the Vercel AI Gateway, `VIV_GATEWAY_URL` to point it elsewhere).
//!
//! Model and provider:
//! - `VIV_MODEL` (with `VIV_PROVIDER`, any provider id of the old registry or `vercel`) wins when set.
//!   Without `VIV_PROVIDER` the first of `anthropic`, `vercel`, `openai`, `google` that has a key is used.
//! - Otherwise the old app's `~/.viv/agent/settings.json`: the last model used, then the default model,
//!   whichever has a key. `VIV_PROVIDER` alone narrows that to one provider.
//! - `OPENAI_BASE_URL` with provider `openai` talks to a local OpenAI-compatible server
//!   (Ollama, llama.cpp) over chat completions; its key is optional.

use std::sync::Arc;

use serde_json::Value;
use vivianne_ai::ai::env_api_keys::get_env_api_key;
use vivianne_ai::ai::provider_registry::provider_spec;
use vivianne_ai::host::{settings_blocking, stored_api_key};

use crate::ai_provider::AiProvider;
use crate::Provider;

const DEFAULT_CHOICES: [&str; 4] = ["anthropic", "vercel", "openai", "google"];

/// The registry's id for an OpenAI-compatible server at a custom address.
const LOCAL_PROVIDER: &str = "local";

/// A provider and one of its models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub provider: String,
    pub model: String,
}

pub fn model() -> Result<String, String> {
    selection().map(|s| s.model)
}

pub fn provider() -> Result<Arc<dyn Provider>, String> {
    provider_for(&selection()?.provider)
}

/// The provider with this id, using the saved or environment key for it.
pub fn provider_for(id: &str) -> Result<Arc<dyn Provider>, String> {
    if id == "openai"
        && let Ok(base) = std::env::var("OPENAI_BASE_URL")
    {
        return Ok(Arc::new(AiProvider::new(LOCAL_PROVIDER, Some(base), key_for("openai"))));
    }
    let key = key_for(id).ok_or_else(|| format!("no API key for `{id}`; save one in Vivianne or set its variable"))?;
    let base = (id == "vercel").then(|| std::env::var("VIV_GATEWAY_URL").ok()).flatten();
    Ok(Arc::new(AiProvider::new(id, base, Some(key))))
}

/// The provider and model to use now.
pub fn selection() -> Result<Selection, String> {
    let wanted = std::env::var("VIV_PROVIDER").ok();
    if let Ok(model) = std::env::var("VIV_MODEL") {
        let provider = wanted.or_else(first_provider_with_key).ok_or_else(missing_provider)?;
        return Ok(Selection { provider, model });
    }
    from_settings(&settings_blocking(), wanted.as_deref())
        .ok_or_else(|| "set VIV_MODEL, or pick a model with a saved key in Vivianne".to_owned())
}

/// The models to fall back to, in order, from the old app's settings (`fallbackModel`, then the
/// `fallbackModels` list). Entries whose provider has no key are left out.
#[must_use]
pub fn fallbacks() -> Vec<Selection> {
    fallbacks_from(&settings_blocking()).into_iter().filter(|s| is_usable(&s.provider)).collect()
}

/// Whether passing failures are retried (`autoRetryEnabled`, on unless the user turned it off).
pub fn auto_retry() -> bool {
    settings_blocking().get("autoRetryEnabled").and_then(Value::as_bool).unwrap_or(true)
}

fn fallbacks_from(settings: &Value) -> Vec<Selection> {
    let single = settings.get("fallbackModel").into_iter();
    let list = settings.get("fallbackModels").and_then(Value::as_array).into_iter().flatten();
    let mut chain: Vec<Selection> = Vec::new();
    for entry in single.chain(list) {
        if let Some(found) = pair(entry.get("provider"), entry.get("id"))
            && !chain.contains(&found)
        {
            chain.push(found);
        }
    }
    chain
}

fn missing_provider() -> String {
    "set VIV_PROVIDER and a key (for example ANTHROPIC_API_KEY)".to_owned()
}

fn first_provider_with_key() -> Option<String> {
    DEFAULT_CHOICES.iter().find(|id| key_for(id).is_some()).map(|id| (*id).to_owned())
}

/// The old app's last model, then its default model: the first whose provider is known and has a key.
fn from_settings(settings: &Value, only: Option<&str>) -> Option<Selection> {
    let last = pair(settings.pointer("/lastModel/provider"), settings.pointer("/lastModel/id"));
    let default = pair(settings.get("defaultProvider"), settings.get("defaultModel"));
    [last, default]
        .into_iter()
        .flatten()
        .filter(|s| only.is_none_or(|p| p == s.provider))
        .find(|s| is_usable(&s.provider))
}

fn pair(provider: Option<&Value>, model: Option<&Value>) -> Option<Selection> {
    let text = |v: Option<&Value>| v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    Some(Selection { provider: text(provider)?, model: text(model)? })
}

fn is_usable(provider: &str) -> bool {
    (provider == "vercel" || provider_spec(provider).is_some()) && key_for(provider).is_some()
}

fn key_for(provider: &str) -> Option<String> {
    stored_api_key(provider).or_else(|| match provider {
        "vercel" => std::env::var("AI_GATEWAY_API_KEY").ok(),
        other => get_env_api_key(other, None),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_provider_nobody_knows_has_no_key() {
        assert_eq!(key_for("no-such-provider"), None);
        assert!(!is_usable("no-such-provider"));
    }

    #[test]
    fn settings_pairs_need_both_a_provider_and_a_model() {
        let s = json!({"lastModel": {"id": "gpt-5-nano", "provider": "openai"}, "defaultProvider": "nvidia"});
        assert_eq!(
            pair(s.pointer("/lastModel/provider"), s.pointer("/lastModel/id")),
            Some(Selection { provider: "openai".into(), model: "gpt-5-nano".into() })
        );
        assert_eq!(pair(s.get("defaultProvider"), s.get("defaultModel")), None);
        assert_eq!(pair(Some(&json!("")), Some(&json!("m"))), None);
    }

    #[test]
    fn the_fallback_chain_reads_the_single_entry_first_then_the_list_without_repeats() {
        let s = json!({
            "fallbackModel": {"provider": "openai", "id": "a"},
            "fallbackModels": [{"provider": "openai", "id": "a"}, {"provider": "google", "id": "b"}, {"id": "no-provider"}, null]
        });
        let got: Vec<_> = fallbacks_from(&s).into_iter().map(|f| (f.provider, f.model)).collect();
        assert_eq!(got, [("openai".to_owned(), "a".to_owned()), ("google".to_owned(), "b".to_owned())]);
        assert!(fallbacks_from(&json!({})).is_empty());
    }

    #[test]
    fn settings_choose_nothing_when_no_provider_has_a_key() {
        let s = json!({"lastModel": {"id": "m", "provider": "no-such-provider"}});
        assert_eq!(from_settings(&s, None), None);
    }
}
