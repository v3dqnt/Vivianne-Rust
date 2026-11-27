//! The agent a subagent of Vivianne's own runs as, the same on every surface (R1.4).

use vivianne_core::Mode;
use vivianne_providers::env;
use vivianne_tools::Toolbox;

use super::{AgentSpec, Spawner};
use crate::Agent;
use crate::sections::order;

/// A read-only subagent runs in Plan mode, for what Plan mode allows. The person's Plan mode text would tell it to
/// write a plan and stop, which is not what it was started for.
const READ_ONLY: &str = "# Read-only
You can only read and search: change no project files. Do the task you were given, not a plan for it, and answer with what you found. The task board tools, if you have them, still work: claim and complete your tasks.";

fn set_standing(agent: &Agent, mode: Mode) {
    agent.set_mode(mode);
    if mode == Mode::Plan {
        // Replaces the built-in section of the same name.
        agent.set_section("mode", order::MODE, READ_ONLY);
    }
}

/// Builds subagents from the keys and settings on this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Standard;

impl Spawner for Standard {
    fn build(&self, spec: &AgentSpec) -> Result<Agent, String> {
        let (provider, model) = spec
            .model
            .clone()
            .ok_or("there is no model to give the subagent")?;
        // The parent's own connection when there is one, so a subagent bills and reaches exactly as its parent does.
        let provider = match &spec.provider {
            Some(parent) => parent.0.clone(),
            // An OpenAI-compatible server at its own address answers as `local`, and is asked for as `openai`.
            None => env::provider_for(if provider == "local" {
                "openai"
            } else {
                &provider
            })?,
        };
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
        set_standing(&agent, spec.mode);
        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use vivianne_core::{Message, Role, Thinking};
    use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};

    use super::*;

    /// Keeps the system prompt it was last sent.
    #[derive(Default)]
    struct Records(Mutex<String>);

    #[async_trait]
    impl Provider for Records {
        fn id(&self) -> &'static str {
            "records"
        }

        async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
            *self.0.lock().unwrap() = request.system.clone().unwrap_or_default();
            Ok(Box::pin(futures::stream::iter([Ok(ChatEvent::Done)])))
        }
    }

    #[tokio::test]
    async fn a_read_only_subagent_is_told_to_do_its_task_not_to_write_a_plan() {
        let provider = Arc::new(Records::default());
        let agent = Agent::new(
            provider.clone(),
            "m",
            None,
            Toolbox::new(std::path::Path::new(".")),
        );
        set_standing(&agent, Mode::Plan);
        let (events, _seen) = tokio::sync::mpsc::unbounded_channel();
        let mut history = vec![Message::new(Role::User, "go")];
        agent
            .run(&mut history, &events, Thinking::Off)
            .await
            .expect("runs");
        let told = provider.0.lock().unwrap().clone();
        assert!(told.contains("# Read-only"), "{told}");
        assert!(!told.contains("PLAN MODE"), "{told}");
    }
}
