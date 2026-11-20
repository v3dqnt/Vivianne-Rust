//! Builds the agent a thread talks to. Configuration comes from the environment
//! for now (see `vivianne_providers::env`); settings and the keychain replace it later.

use std::sync::Arc;

use tokio::runtime::Runtime;
use vivianne_ai::host::ModelRef;
use vivianne_providers::env::{self, Selection};
use vivianne_providers::Provider;
use vivianne_runtime::resilience::RetryPolicy;
use vivianne_runtime::{Agent, ContextSource, Target, coding_system_prompt};
use vivianne_tools::Toolbox;

use crate::chat::ChatSession;
use crate::demo::Demo;

/// A ready session and the provider and model it uses, or what is wrong with the setup.
pub type Setup = Result<(ChatSession, Selection), String>;

/// One session per thread, all sharing the same tokio runtime and project folder.
pub fn new_session(runtime: &Arc<Runtime>, workspace: &std::path::Path) -> Setup {
    let (provider, selection) = if std::env::var_os("VIV_DEMO").is_some() {
        let demo: Arc<dyn Provider> = Arc::new(Demo);
        (demo, Selection { provider: "demo".to_owned(), model: "demo".to_owned() })
    } else {
        let selection = env::selection().map_err(|p| hint(&p))?;
        (env::provider_for(&selection.provider).map_err(|p| hint(&p))?, selection)
    };
    let fallbacks = if selection.provider == "demo" { Vec::new() } else { fallback_targets(&selection) };
    let agent = build_agent(provider, &selection.model, workspace, fallbacks);
    Ok((ChatSession::new(agent, runtime.clone(), workspace.to_path_buf()), selection))
}

/// An agent for `model` of `provider`, for switching a thread to another model.
pub fn agent_for(provider: &str, model: &str, workspace: &std::path::Path) -> Result<Agent, String> {
    let chosen = Selection { provider: provider.to_owned(), model: model.to_owned() };
    Ok(build_agent(env::provider_for(provider)?, model, workspace, fallback_targets(&chosen)))
}

/// An agent that is never asked: a session needs one, but a coding agent answers instead.
pub fn placeholder_agent(workspace: &std::path::Path) -> Result<Agent, String> {
    Ok(build_agent(Arc::new(Demo), "none", workspace, Vec::new()))
}

fn build_agent(provider: Arc<dyn Provider>, model: &str, workspace: &std::path::Path, fallbacks: Vec<Target>) -> Agent {
    let retry = if env::auto_retry() { RetryPolicy::default() } else { RetryPolicy::once() };
    let mut tools = Toolbox::coding(workspace);
    let root = workspace.display().to_string();
    // The project's memory: tools the model can use, and the standing facts that are in every prompt.
    if let Some(backend) = crate::backend::get() {
        let reflecting = ModelRef { id: model.to_owned(), name: model.to_owned(), provider: provider.id().to_owned(), ..ModelRef::default() };
        vivianne_memory::agent::add_tools(&mut tools, &root, &backend.events, reflecting);
    }
    Agent::new(provider, model, Some(coding_system_prompt(workspace)), tools)
        .with_context(ContextSource::new(move || {
            let root = root.clone();
            Box::pin(async move { vivianne_memory::agent::core_block(&root).await })
        }))
        .with_instructions(vec![workspace.display().to_string()])
        .with_fallbacks(fallbacks)
        .with_retry(retry)
}

/// The saved fallback models other than the one chosen, as places to send a request.
fn fallback_targets(chosen: &Selection) -> Vec<Target> {
    env::fallbacks()
        .into_iter()
        .filter(|fallback| fallback != chosen)
        .filter_map(|fallback| Some(Target::new(env::provider_for(&fallback.provider).ok()?, &fallback.model)))
        .collect()
}

fn hint(problem: &str) -> String {
    format!(
        "Not set up: {problem}. Set VIV_MODEL and a provider key, or VIV_DEMO=1 to try it offline."
    )
}
