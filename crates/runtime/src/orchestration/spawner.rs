//! The agent a subagent of Vivianne's own runs as, the same on every surface (R1.4).

use vivianne_providers::env;
use vivianne_tools::Toolbox;

use super::{AgentSpec, Spawner};
use crate::Agent;

/// Builds subagents from the keys and settings on this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Standard;

impl Spawner for Standard {
    fn build(&self, spec: &AgentSpec) -> Result<Agent, String> {
        let (provider, model) = spec
            .model
            .clone()
            .ok_or("there is no model to give the subagent")?;
        // An OpenAI-compatible server at its own address answers as `local`, and is asked for as `openai`.
        let id = if provider == "local" {
            "openai"
        } else {
            &provider
        };
        let provider = env::provider_for(id)?;
        let settings = crate::config::resolve(&spec.folder, None);
        let mut tools = Toolbox::coding(&spec.folder);
        // The same tools and sandbox mode, without the tools that start more agents.
        if let Some(warning) = crate::surface::prepare_child(&mut tools, &spec.folder, &settings) {
            vivianne_core::debuglog::log("subagent", warning);
        }
        let system = format!(
            "You are a subagent working in {}. Do the task you are given and answer with what you found or did.\n{}",
            spec.folder.display(),
            spec.system
        );
        let agent = Agent::new(provider, &model, Some(system), tools)
            .with_max_steps(settings.max_agent_steps.value)
            .with_instructions(vec![spec.folder.display().to_string()]);
        agent.set_mode(spec.mode);
        Ok(agent)
    }
}
