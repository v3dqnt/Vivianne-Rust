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
#[rustfmt::skip]
pub mod code;
#[rustfmt::skip]
pub mod files;
#[rustfmt::skip]
pub mod connections;
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, dead_code, unused, missing_debug_implementations)]
pub mod dap;
#[rustfmt::skip]
mod lsp;
#[rustfmt::skip]
mod lsp_nav;
mod ocr;
pub mod schemas;

mod files_simple;
mod shell;
mod simple;
mod workspace;

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

/// The set of tools one agent may use, and the folder they work in.
pub struct Toolbox {
    cwd: String,
    tools: Vec<Box<dyn Tool>>,
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
        }
    }

    /// The coding tools, working in `root`.
    #[must_use]
    pub fn coding(root: &Path) -> Self {
        let mut toolbox = Self::new(root);
        let cwd = toolbox.cwd.clone();
        let workspace = workspace::Workspace::new(root);
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
        toolbox.add(lsp_nav::LspTool::new(cwd));
        toolbox.add(shell::Shell::new(workspace.clone()));
        toolbox.add(files_simple::WriteFile::new(workspace.clone()));
        toolbox.add(files_simple::EditFile::new(workspace));
        toolbox.add(results::ReadToolResult);
        toolbox
    }

    pub fn add(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Box::new(tool));
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
        let tool = self.get(name)?;
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

// The copied code refers to the original app's module paths. These re-exports
// and the small stand-ins below make those paths resolve without editing it.
pub mod agent {
    pub mod orchestration;
    pub mod thread_msgs;

    pub mod tools {
        pub use crate::{builtin, code, files};

        pub mod desktop {
            pub(crate) use crate::ocr;
        }

        pub mod exec;
    }

    pub mod system {
        pub mod paths;
    }

    pub mod runtime {
        pub mod commands;
        #[rustfmt::skip]
        pub mod streaming;
    }
}

pub mod ai;
pub mod mods;
#[rustfmt::skip]
pub mod proc;
pub mod dev_server;
pub mod mcp;
pub mod open_apps;
pub mod results;
mod tauri;
