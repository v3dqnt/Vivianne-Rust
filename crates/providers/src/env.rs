//! Pick a provider and model from environment variables.
//! Temporary configuration source: settings and the keychain replace it later.
//!
//! - `VIV_MODEL`: model name (required). For `vercel`, `provider/model`, e.g. `anthropic/claude-sonnet-4`.
//! - `VIV_PROVIDER`: `anthropic`, `vercel` or `openai`. Default: `anthropic` if `ANTHROPIC_API_KEY`
//!   is set, else `vercel` if `AI_GATEWAY_API_KEY` is set, else `openai`.
//! - `ANTHROPIC_API_KEY`; `AI_GATEWAY_API_KEY` (Vercel AI Gateway), `VIV_GATEWAY_URL` to point at another
//!   gateway; `OPENAI_API_KEY` (optional for local servers), `OPENAI_BASE_URL`.

use std::sync::Arc;

use crate::Provider;
use crate::legacy::Legacy;
use crate::openai::OpenAiCompatible;

const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";
/// The Vercel AI Gateway speaks the OpenAI chat-completions API, so the OpenAI adapter drives it.
const DEFAULT_GATEWAY_BASE: &str = "https://ai-gateway.vercel.sh/v1";

pub fn model() -> Result<String, String> {
    std::env::var("VIV_MODEL").map_err(|_| "set VIV_MODEL".to_owned())
}

pub fn provider() -> Result<Arc<dyn Provider>, String> {
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let gateway_key = std::env::var("AI_GATEWAY_API_KEY").ok();
    let wanted = std::env::var("VIV_PROVIDER").unwrap_or_else(|_| {
        if anthropic_key.is_some() {
            "anthropic"
        } else if gateway_key.is_some() {
            "vercel"
        } else {
            "openai"
        }
        .to_owned()
    });
    match wanted.as_str() {
        "anthropic" => {
            let key = anthropic_key.ok_or("set ANTHROPIC_API_KEY")?;
            Ok(Arc::new(Legacy::new("anthropic", None, Some(key))))
        }
        "vercel" => {
            let key = gateway_key.ok_or("set AI_GATEWAY_API_KEY")?;
            let base = std::env::var("VIV_GATEWAY_URL")
                .unwrap_or_else(|_| DEFAULT_GATEWAY_BASE.to_owned());
            Ok(Arc::new(Legacy::new("vercel", Some(base), Some(key))))
        }
        "openai" => {
            let base =
                std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_OPENAI_BASE.to_owned());
            let key = std::env::var("OPENAI_API_KEY").ok();
            Ok(Arc::new(OpenAiCompatible::new("openai", &base, key)))
        }
        other => Err(format!("unknown VIV_PROVIDER `{other}`")),
    }
}
