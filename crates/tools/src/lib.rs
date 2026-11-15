//! Agent tools. Each tool describes itself (name, description, JSON schema)
//! and runs from JSON arguments. Desktop control (`dip`) and the browser arrive later.

mod files;
mod shell;
mod workspace;

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

pub use files::{EditFile, ReadFile, WriteFile};
pub use shell::Shell;
pub use workspace::Workspace;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("bad arguments: {0}")]
    BadArgs(String),
    #[error("{0}")]
    Failed(String),
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema of the arguments.
    fn parameters(&self) -> Value;
    async fn call(&self, args: &Value) -> Result<String, ToolError>;
}

/// The set of tools one agent may use.
#[derive(Default)]
pub struct Toolbox {
    tools: Vec<Box<dyn Tool>>,
}

impl std::fmt::Debug for Toolbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.tools.iter().map(|t| t.name()))
            .finish()
    }
}

impl Toolbox {
    /// Shell plus file read/write/edit, all confined to `root`.
    #[must_use]
    pub fn coding(root: &Path) -> Self {
        let ws = Workspace::new(root);
        let mut toolbox = Self::default();
        toolbox.add(Shell::new(ws.clone()));
        toolbox.add(ReadFile::new(ws.clone()));
        toolbox.add(WriteFile::new(ws.clone()));
        toolbox.add(EditFile::new(ws));
        toolbox
    }

    pub fn add(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Box::new(tool));
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Tool> {
        self.tools.iter().map(AsRef::as_ref)
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.iter().find(|t| t.name() == name)
    }
}

pub(crate) fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args[key]
        .as_str()
        .ok_or_else(|| ToolError::BadArgs(format!("`{key}` must be a string")))
}
