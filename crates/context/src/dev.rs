//! Telling the model where this app's own source lives, without putting the path in the system prompt. Port of
//! `harness-dev-context`: the prompt carries only guidance with no value in it; the checkout path arrives as runtime context.
//! A process started from another checkout or installation then never changes the saved system prompt.

/// Prompt-section and runtime-context name for the implementation checkout.
pub const SOURCE_NAME: &str = "vivianne:source";

const SOURCE_GUIDANCE: &str = "The checkout location and current working directory are separate values and may differ; never infer the working directory from the checkout path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend Vivianne itself.";

/// The value-free paragraph for the system prompt.
#[must_use]
pub fn source_prompt() -> String {
    format!("The runtime context names the Vivianne implementation checkout. {SOURCE_GUIDANCE}")
}

/// The runtime-context line naming the checkout, or `None` when this build has none to name (an installed build).
#[must_use]
pub fn source_context(checkout: Option<&str>) -> Option<String> {
    let path = checkout.filter(|p| !p.is_empty())?;
    Some(format!(
        "Current Vivianne implementation checkout: {}.",
        serde_json::to_string(path).unwrap_or_default()
    ))
}

/// The checkout this binary was built from, when it still exists on this machine.
#[must_use]
pub fn built_from() -> Option<String> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent()?.parent()?;
    root.join("Cargo.toml")
        .is_file()
        .then(|| root.display().to_string())
}
