//! Debugging a running program through a debug adapter (the Debug Adapter Protocol), for the agent.
//!
//! `config` says which adapter runs which program and how, `client` speaks the protocol to one adapter, and `session` keeps what is known about
//! the program being debugged. The agent reaches it through the `debug` and `debug_run` tools (`agent/tools/debug.rs`).
//!
//! Ported in design from oh-my-pi's debugger (its maps are in `docs/omp-dap-map.md`), not in code: that is TypeScript. What it does differently is
//! listed in `docs/c4-omp-lsp-dap/c4-component-dap.md`: breakpoints before `configurationDone`, adapters from a fixed table (never from the
//! project), no program started on an adapter's say-so, one session per thread, and a wait that runs out says the program is still running.

pub mod client;
pub mod config;
pub mod schemas;
pub mod session;
pub mod tools;
pub mod ui;

use async_trait::async_trait;
use serde_json::Value;

use crate::builtin::{Tool, ToolResult};

/// `debug` (looks) or `debug_run` (starts, steers, ends). One debug session per workspace: the workspace folder is its key,
/// which is also how the Debug panel finds it.
pub struct DebugTool {
    cwd: String,
    run: bool,
}

impl DebugTool {
    #[must_use]
    pub fn new(cwd: String, run: bool) -> Self {
        Self { cwd, run }
    }
}

#[async_trait]
impl Tool for DebugTool {
    fn name(&self) -> &str {
        if self.run { "debug_run" } else { "debug" }
    }

    fn definition(&self) -> Option<Value> {
        Some(if self.run { schemas::debug_run() } else { schemas::debug() })
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let result = if self.run {
            tools::run(args, &self.cwd, &self.cwd, None).await
        } else {
            tools::read(args, &self.cwd).await
        };
        match result {
            Ok(content) => ToolResult { content, is_error: false, details: None },
            Err(e) => ToolResult { content: e, is_error: true, details: None },
        }
    }
}
