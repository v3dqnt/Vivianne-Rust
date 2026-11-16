//! Pick a provider and model from environment variables.
//! Temporary configuration source: the Settings page and the keychain replace it.
//!
//! - `VIV_MODEL`: model name (required). For `vercel`, `provider/model`, e.g. `anthropic/claude-sonnet-4`.
//! - `VIV_PROVIDER`: any provider id of the old registry (`anthropic`, `openai`, `google`, `groq`...)
//!   or `vercel`. Default: the first of `anthropic`, `vercel`, `openai`, `google` whose key is set.
//! - Keys are the ones the old app read: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`...
//!   and `AI_GATEWAY_API_KEY` for the Vercel AI Gateway (`VIV_GATEWAY_URL` points it elsewhere).
//! - `OPENAI_BASE_URL` with `VIV_PROVIDER=openai` talks to a local OpenAI-compatible server
//!   (Ollama, llama.cpp) over chat completions; its key is optional.

use std::sync::Arc;

use vivianne_ai::ai::env_api_keys::get_env_api_key;

use crate::Provider;
use crate::ai_provider::AiProvider;

const DEFAULT_CHOICES: [&str; 4] = ["anthropic", "vercel", "openai", "google"];

/// The registry's id for an OpenAI-compatible server at a custom address.
const LOCAL_PROVIDER: &str = "local";

pub fn model() -> Result<String, String> {
    std::env::var("VIV_MODEL").map_err(|_| "set VIV_MODEL".to_owned())
}

pub fn provider() -> Result<Arc<dyn Provider>, String> {
    let id = wanted_provider()?;
    if id == "openai"
        && let Ok(base) = std::env::var("OPENAI_BASE_URL")
    {
        return Ok(Arc::new(AiProvider::new(
            LOCAL_PROVIDER,
            Some(base),
            key_for("openai"),
        )));
    }
    let key = key_for(&id).ok_or_else(|| format!("no API key for `{id}`; set its key variable"))?;
    let base = (id == "vercel")
        .then(|| std::env::var("VIV_GATEWAY_URL").ok())
        .flatten();
    Ok(Arc::new(AiProvider::new(&id, base, Some(key))))
}

fn wanted_provider() -> Result<String, String> {
    if let Ok(id) = std::env::var("VIV_PROVIDER") {
        return Ok(id);
    }
    DEFAULT_CHOICES
        .iter()
        .find(|id| key_for(id).is_some())
        .map(|id| (*id).to_owned())
        .ok_or_else(|| "set VIV_PROVIDER and a key (for example ANTHROPIC_API_KEY)".to_owned())
}

fn key_for(provider: &str) -> Option<String> {
    match provider {
        "vercel" => std::env::var("AI_GATEWAY_API_KEY").ok(),
        other => get_env_api_key(other, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vercel_gateway_reads_its_own_key_variable() {
        // Not set in the test environment unless the machine has one; either way no panic.
        assert_eq!(key_for("vercel"), std::env::var("AI_GATEWAY_API_KEY").ok());
    }

    #[test]
    fn an_unknown_provider_has_no_key() {
        assert_eq!(key_for("no-such-provider"), None);
    }
}
