//! Builds the agent a thread talks to. Configuration comes from the environment
//! for now (see `vivianne_providers::env`); settings and the keychain replace it later.

use std::sync::Arc;

use tokio::runtime::Runtime;
use vivianne_providers::env::{self, Selection};
use vivianne_providers::Provider;
use vivianne_runtime::{Agent, CODING_SYSTEM_PROMPT};
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
    let agent = build_agent(provider, &selection.model, workspace);
    Ok((ChatSession::new(agent, runtime.clone()), selection))
}

/// An agent for `model` of `provider`, for switching a thread to another model.
pub fn agent_for(provider: &str, model: &str, workspace: &std::path::Path) -> Result<Agent, String> {
    Ok(build_agent(env::provider_for(provider)?, model, workspace))
}

fn build_agent(provider: Arc<dyn Provider>, model: &str, workspace: &std::path::Path) -> Agent {
    Agent::new(provider, model, Some(CODING_SYSTEM_PROMPT.to_owned()), Toolbox::coding(workspace))
}

fn hint(problem: &str) -> String {
    format!(
        "Not set up: {problem}. Set VIV_MODEL and a provider key, or VIV_DEMO=1 to try it offline."
    )
}
