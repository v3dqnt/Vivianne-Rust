//! Builds the agent a thread talks to. Configuration comes from the environment
//! for now (see `vivianne_providers::env`); settings and the keychain replace it later.

use std::sync::Arc;

use tokio::runtime::Runtime;
use vivianne_providers::{Provider, env};
use vivianne_runtime::{Agent, CODING_SYSTEM_PROMPT};
use vivianne_tools::Toolbox;

use crate::chat::ChatSession;
use crate::demo::Demo;

/// A ready session and the model name it uses, or what is wrong with the setup.
pub type Setup = Result<(ChatSession, String), String>;

/// One session per thread, all sharing the same tokio runtime and project folder.
pub fn new_session(runtime: &Arc<Runtime>, workspace: &std::path::Path) -> Setup {
    let (provider, model): (Arc<dyn Provider>, String) = if std::env::var_os("VIV_DEMO").is_some() {
        (Arc::new(Demo), "demo".to_owned())
    } else {
        (
            env::provider().map_err(|p| hint(&p))?,
            env::model().map_err(|p| hint(&p))?,
        )
    };
    let agent = Agent::new(
        provider,
        &model,
        Some(CODING_SYSTEM_PROMPT.to_owned()),
        Toolbox::coding(workspace),
    );
    Ok((ChatSession::new(agent, runtime.clone()), model))
}

fn hint(problem: &str) -> String {
    format!(
        "Not set up: {problem}. Set VIV_MODEL and a provider key, or VIV_DEMO=1 to try it offline."
    )
}
