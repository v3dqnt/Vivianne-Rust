//! What the server needs from the machine it runs on: an agent for a folder, and where conversations are kept.

use std::path::Path;
use std::sync::Arc;

use vivianne_core::Message;
use vivianne_runtime::store::{self, SavedThread};
use vivianne_runtime::{Agent, title};
use vivianne_tools::Toolbox;

/// An agent and what to record it as.
pub struct Built {
    pub agent: Agent,
    pub provider: String,
    pub model: String,
}

impl std::fmt::Debug for Built {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Built")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// Saved conversations. Behind a trait so a test touches no real one (R7).
pub trait Threads: Send + Sync {
    fn save(&self, thread: &SavedThread) -> Result<(), String>;
    fn load(&self, id: &str) -> Option<SavedThread>;
    /// `(folder, id)` of the saved conversations, newest first; only `folder`'s when one is given.
    fn list(&self, folder: Option<&str>) -> Vec<(String, String)>;
    /// The right to write a conversation, held as long as the value lives.
    fn lease(&self, id: &str) -> Result<Box<dyn Send>, String>;
}

/// The person's own conversations, the ones the app shows.
#[derive(Debug, Default)]
pub struct Standard;

impl Threads for Standard {
    fn save(&self, thread: &SavedThread) -> Result<(), String> {
        store::save(thread)
    }

    fn load(&self, id: &str) -> Option<SavedThread> {
        store::load(id)
    }

    fn list(&self, folder: Option<&str>) -> Vec<(String, String)> {
        match folder {
            Some(folder) => store::list(folder)
                .into_iter()
                .map(|s| (folder.to_owned(), s.id))
                .collect(),
            None => store::list_everywhere()
                .into_iter()
                .map(|(folder, s)| (folder, s.id))
                .collect(),
        }
    }

    fn lease(&self, id: &str) -> Result<Box<dyn Send>, String> {
        store::lease(id)
            .map(|lease| Box::new(lease) as Box<dyn Send>)
            .map_err(|e| e.to_string())
    }
}

/// Builds the agent for a session's folder.
pub type Build = Arc<dyn Fn(&Path) -> Result<Built, String> + Send + Sync>;

/// How the server gets its agents and keeps its conversations.
#[derive(Clone)]
pub struct Host {
    pub build: Build,
    pub threads: Arc<dyn Threads>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Host")
    }
}

/// The record of a conversation after a turn. What the saved one already says about itself (its name, its mode,
/// where it was branched from) is kept.
pub(super) fn record(
    id: &str,
    folder: &str,
    built: (&str, &str),
    history: &[Message],
    before: Option<&SavedThread>,
) -> SavedThread {
    let (title, source) = before.filter(|t| !t.title.is_empty()).map_or_else(
        || {
            let (title, source) = title::decide(None, title::first_ask(history));
            (title, source.to_owned())
        },
        |thread| (thread.title.clone(), thread.title_source.clone()),
    );
    SavedThread {
        id: id.to_owned(),
        workspace: before.map_or_else(|| folder.to_owned(), |t| t.workspace.clone()),
        title,
        title_source: source,
        provider: built.0.to_owned(),
        model: built.1.to_owned(),
        mode: before.map_or(vivianne_core::Mode::Ask, |t| t.mode),
        updated_at: store::now(),
        history: history.to_vec(),
        parent: before.and_then(|t| t.parent.clone()),
    }
}

/// The agent the command line would run in `folder`, with the same tools and settings as every surface (R1.4).
pub fn standard_build(folder: &Path, system: String) -> Result<Built, String> {
    let model = vivianne_providers::env::model()?;
    let provider = vivianne_providers::env::provider()?;
    let id = provider.id().to_owned();
    let settings = vivianne_runtime::config::resolve(folder, None);
    vivianne_tools::results::set_limit(settings.max_tool_result_bytes.value);
    let mut toolbox = Toolbox::coding(folder);
    if let Some(warning) = vivianne_runtime::surface::prepare(&mut toolbox, folder, &settings) {
        // Standard output carries only the protocol.
        eprintln!("viv: {warning}");
    }
    let mut agent = Agent::new(provider, &model, Some(system), toolbox)
        .with_max_steps(settings.max_agent_steps.value)
        .with_instructions(vec![folder.display().to_string()]);
    if let Ok(recorder) = vivianne_runtime::changes::Recorder::new(folder, settings.change_limits())
    {
        agent = agent.with_changes(Arc::new(recorder));
    }
    Ok(Built {
        agent,
        provider: id,
        model,
    })
}
