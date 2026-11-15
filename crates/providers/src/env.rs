//! Pick a provider and model from environment variables.
//! Temporary configuration source: settings and the keychain replace it later.
//!
//! - `VIV_MODEL`: model name (required)
//! - `VIV_PROVIDER`: `anthropic` (default when `ANTHROPIC_API_KEY` is set) or `openai`
//! - `ANTHROPIC_API_KEY`; `OPENAI_API_KEY` (optional for local servers), `OPENAI_BASE_URL`

use std::sync::Arc;

use crate::Provider;
use crate::anthropic::Anthropic;
use crate::openai::OpenAiCompatible;

const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";

pub fn model() -> Result<String, String> {
    std::env::var("VIV_MODEL").map_err(|_| "set VIV_MODEL".to_owned())
}

pub fn provider() -> Result<Arc<dyn Provider>, String> {
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let wanted = std::env::var("VIV_PROVIDER").unwrap_or_else(|_| {
        if anthropic_key.is_some() {
            "anthropic"
        } else {
            "openai"
        }
        .to_owned()
    });
    match wanted.as_str() {
        "anthropic" => {
            let key = anthropic_key.ok_or("set ANTHROPIC_API_KEY")?;
            Ok(Arc::new(Anthropic::new(key)))
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
