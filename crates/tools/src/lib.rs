#![recursion_limit = "512"]
//! Agent tools. The read-and-search tools (`read`, `ls`, `grep`, `glob`,
//! `code_search`, `repo_clone`, `repo_overview`, `diagnostics`, `lsp`) are Vivianne's originals,
//! copied as they were (see the README here). `shell`, `write` and `edit` are
//! simple stand-ins until their originals are copied too.
//!
//! A tool is `name` + `execute(args, cwd) -> ToolResult`; the model-facing
//! definitions live in `schemas`, exactly as in the original app.

// The copied modules are marked `rustfmt::skip` so formatting never touches them.
#[rustfmt::skip]
pub mod builtin;
pub mod browser;
pub mod computer;
pub mod git;
pub mod present;
pub mod ptc;
pub mod run_code;
pub mod validate;
#[cfg(test)]
mod testing;
#[rustfmt::skip]
pub mod code;
#[rustfmt::skip]
pub mod files;
#[rustfmt::skip]
pub mod connections;
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, missing_debug_implementations)]
pub mod dap;
#[rustfmt::skip]
mod lsp;
pub mod schemas;

mod simple;

use std::path::Path;

use serde_json::Value;

pub use builtin::{Tool, ToolResult};

/// A tool as the model sees it.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// What a condition on a tool can look at: the model the next step will ask.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepInfo {
    pub provider: String,
    pub model: String,
}

/// Decides, for each model step, whether a tool is offered and callable.
pub type Condition = std::sync::Arc<dyn Fn(&StepInfo) -> bool + Send + Sync>;

/// The set of tools one agent may use, and the folder they work in.
pub struct Toolbox {
    cwd: String,
    tools: Vec<Box<dyn Tool>>,
    /// Tools that are offered only while their condition holds, by name.
    conditions: std::collections::HashMap<String, Condition>,
    /// The step the conditions are judged against, set by the loop before each model call.
    step: std::sync::Mutex<StepInfo>,
}

impl std::fmt::Debug for Toolbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.tools.iter().map(|t| t.name()))
            .finish()
    }
}

/// R3.7: a result over the limit is kept aside and the model gets a preview and a handle. The tool that reads
/// the kept results is exempt, so it can page through one without making a new one.
fn capped(name: &str, mut result: ToolResult) -> ToolResult {
    if name != results::TOOL_NAME {
        result.content = results::cap(std::mem::take(&mut result.content));
    }
    result
}

impl Toolbox {
    /// An empty box working in `cwd`.
    #[must_use]
    pub fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_string_lossy().into_owned(),
            tools: Vec::new(),
            conditions: std::collections::HashMap::new(),
            step: std::sync::Mutex::new(StepInfo::default()),
        }
    }

    /// The coding tools, working in `root`.
    #[must_use]
    pub fn coding(root: &Path) -> Self {
        let mut toolbox = Self::new(root);
        let cwd = toolbox.cwd.clone();
        let workspace = simple::Workspace::new(root);
        toolbox.add(builtin::ReadTool::new(cwd.clone()));
        toolbox.add(builtin::LsTool::new(cwd.clone()));
        toolbox.add(builtin::GrepTool::new(cwd.clone()));
        toolbox.add(builtin::CodeSearchTool::new(cwd.clone()));
        toolbox.add(builtin::GlobTool::new(cwd.clone()));
        toolbox.add(code::repo_tools::RepoCloneTool::new());
        toolbox.add(code::repo_tools::RepoOverviewTool::new(cwd.clone()));
        toolbox.add(builtin::DiagnosticsTool::new(cwd.clone()));
        toolbox.add(dap::DebugTool::new(cwd.clone(), false));
        toolbox.add(dap::DebugTool::new(cwd.clone(), true));
        toolbox.add(lsp::nav::LspTool::new(cwd));
        toolbox.add(simple::Shell::new(workspace.clone()));
        toolbox.add(simple::WriteFile::new(workspace.clone()));
        toolbox.add(simple::EditFile::new(workspace));
        toolbox.add(results::ReadToolResult);
        toolbox.add(present::Present::new(root));
        toolbox.add(run_code::RunCode::new(root));
        // Only where a LibreOffice is installed: a tool that cannot work is not offered.
        if files::office_pdf::tool::available() {
            toolbox.add(files::office_pdf::tool::ConvertToPdf::new(root));
        }
        toolbox
    }

    pub fn add(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Box::new(tool));
    }

    /// Adds a tool that is offered, and callable, only on steps where `condition` holds (for a tool that suits one
    /// model and not another).
    pub fn add_when(&mut self, tool: impl Tool + 'static, condition: Condition) {
        self.conditions.insert(tool.name().to_owned(), condition);
        self.tools.push(Box::new(tool));
    }

    /// Says which step the conditions are judged against. The agent loop calls this before every model call.
    pub fn set_step(&self, step: StepInfo) {
        *self.step.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = step;
    }

    /// Whether `name` may be offered and called on the current step: no condition, or one that holds.
    #[must_use]
    pub fn is_active(&self, name: &str) -> bool {
        self.conditions.get(name).is_none_or(|condition| {
            let step = self.step.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            condition(&step)
        })
    }

    /// How long a call to `name` may run, when the tool says.
    #[must_use]
    pub fn timeout_of(&self, name: &str) -> Option<std::time::Duration> {
        self.get(name)?.timeout()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(AsRef::as_ref)
    }

    /// Reads the connected MCP servers' tools again if any changed.
    pub async fn refresh(&self) {
        mcp::refresh_tools().await;
    }

    /// Run the named tool, or `None` if there is no such tool.
    pub async fn execute(&self, name: &str, args: &Value) -> Option<ToolResult> {
        if name.starts_with("mcp__") && self.get(name).is_none() {
            // A tool of a connected MCP server: its text is the result, its failure an error result.
            if !mcp::definitions()
                .iter()
                .any(|(public, _, _)| public == name)
            {
                return None;
            }
            let result = match mcp::call_public(name, args.clone()).await {
                Ok(content) => ToolResult {
                    content,
                    is_error: false,
                    details: None,
                },
                Err(content) => ToolResult {
                    content,
                    is_error: true,
                    details: None,
                },
            };
            return Some(capped(name, result));
        }
        let tool = self.get(name).filter(|tool| self.is_active(tool.name()))?;
        Some(capped(name, tool.execute(args, &self.cwd).await))
    }

    /// The definitions of the tools in this box, in the order they were added.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDef> {
        let originals = schemas::definitions();
        let own = simple::definitions();
        let servers = mcp::definitions()
            .into_iter()
            .map(|(name, description, parameters)| ToolDef {
                name,
                description,
                parameters,
            });
        self.tools
            .iter()
            .filter(|tool| self.is_active(tool.name()))
            .filter_map(|tool| {
                let found = tool.definition().or_else(|| {
                    originals
                        .iter()
                        .chain(own.iter())
                        .find(|d| d["name"] == tool.name())
                        .cloned()
                })?;
                Some(ToolDef {
                    name: tool.name().to_owned(),
                    description: found["description"].as_str().unwrap_or_default().to_owned(),
                    parameters: found["parameters"].clone(),
                })
            })
            .chain(servers)
            .collect()
    }
}

pub mod host;
pub mod mcp;
pub mod project;
// Where they were before `project/` existed, so callers keep their paths.
pub use project::{dev_server, open_apps};
#[rustfmt::skip]
pub mod proc;
pub mod results;
