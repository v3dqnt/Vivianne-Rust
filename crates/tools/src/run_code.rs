//! `run_code`: the model writes a small JavaScript program that calls the read-only tools itself, loops, filters and
//! combines what they return, and answers with only the result. Many reads become one call whose output is what the
//! program chose to say, so the conversation carries the answer and not every file the program looked at. This is the
//! programmatic tool executor of the original (`tool-executor-ptc`) on top of the program runtime in [`crate::ptc`].
//!
//! The program runs in an embedded `QuickJS` engine with no file, network, process or module access of its own: the only
//! way out is the generated `tools` object, and that holds only tools that read (`read`, `ls`, `grep`, `code_search`,
//! `glob`), so `run_code` itself only looks and needs no approval. Each tool is an async function: `await` it, or start
//! several and `await Promise.all([...])`. A failed call rejects with a `ToolError` that names the tool. A program has a
//! time limit, a memory limit, a stack limit and a limit on how many tool calls it may make; its result must be lossless
//! JSON.
//!
//! With `VIV_RUN_CODE_STATE=persistent` the tool keeps a REPL environment for the life of the conversation's toolbox:
//! top-level variables survive from one call to the next (see [`ptc::Runtime::execution_instructions`]).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::ptc::{
    self, BindingErrorClass, BindingFunction, BindingNamespace, ExecutionMode, OwnerId, RunRequest,
    Timeout,
};
use crate::{Tool, ToolResult, Toolbox, builtin};

/// The tool's name.
pub const NAME: &str = "run_code";
/// The variable that turns on retained state.
pub const STATE_ENV: &str = "VIV_RUN_CODE_STATE";

/// Wall time of one program, tool calls included.
#[allow(clippy::duration_suboptimal_units)]
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// Tool calls one program may make.
pub const MAX_CALLS: usize = 50;
/// Most bytes of what a program prints and returns.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Runs programs that call read-only tools.
pub struct RunCode {
    inner: Arc<Toolbox>,
    runtime: Arc<ptc::Runtime>,
    owner: OwnerId,
    time_limit: Duration,
    max_calls: usize,
}

impl std::fmt::Debug for RunCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunCode")
            .field("max_calls", &self.max_calls)
            .finish_non_exhaustive()
    }
}

impl RunCode {
    /// Programs that read in `root`; state is kept between calls only if `VIV_RUN_CODE_STATE=persistent`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        let persistent =
            std::env::var(STATE_ENV).is_ok_and(|v| v.eq_ignore_ascii_case("persistent"));
        Self::with_mode(
            root,
            if persistent {
                ExecutionMode::Persistent
            } else {
                ExecutionMode::PerCall
            },
        )
    }

    /// With the execution mode chosen outright (a test's choice).
    #[must_use]
    pub fn with_mode(root: &Path, mode: ExecutionMode) -> Self {
        let cwd = root.to_string_lossy().into_owned();
        let mut inner = Toolbox::new(root);
        inner.add(builtin::ReadTool::new(cwd.clone()));
        inner.add(builtin::LsTool::new(cwd.clone()));
        inner.add(builtin::GrepTool::new(cwd.clone()));
        inner.add(builtin::CodeSearchTool::new(cwd.clone()));
        inner.add(builtin::GlobTool::new(cwd));
        let limit_ms = u64::try_from(TIME_LIMIT.as_millis()).unwrap_or(u64::MAX);
        let mut config = ptc::Config {
            mode,
            timeout_ms: limit_ms,
            max_timeout_ms: limit_ms,
            ..ptc::Config::default()
        };
        config.limits.max_output_bytes = MAX_OUTPUT_BYTES;
        let runtime =
            Arc::new(ptc::Runtime::new(config).unwrap_or_else(|_| {
                unreachable!("a configuration made of positive numbers is valid")
            }));
        let owner = runtime.new_owner();
        Self {
            inner: Arc::new(inner),
            runtime,
            owner,
            time_limit: TIME_LIMIT,
            max_calls: MAX_CALLS,
        }
    }

    /// With other limits (a test's short ones).
    #[must_use]
    pub fn with_limits(mut self, time_limit: Duration, max_calls: usize) -> Self {
        self.time_limit = time_limit;
        self.max_calls = max_calls;
        self
    }

    /// The generated SDK as the model is told it: one line per tool with its arguments.
    fn sdk(&self) -> String {
        self.inner
            .definitions()
            .iter()
            .map(|d| {
                let required: Vec<&str> = d.parameters["required"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                // What is needed first, then what may be left out.
                let optional = d.parameters["properties"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(name, _)| name.as_str())
                    .filter(|name| !required.contains(name));
                let args: Vec<String> = required
                    .iter()
                    .map(|name| (*name).to_owned())
                    .chain(optional.map(|name| format!("{name}?")))
                    .collect();
                format!("await tools.{}({{ {} }})", d.name, args.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The `tools` namespace for one run: each read-only tool as an async function that checks its arguments against the
    /// tool's schema, counts against the program's call limit, and answers with the tool's text or rejects with it.
    fn bindings(&self) -> BindingNamespace {
        let calls = Arc::new(AtomicUsize::new(0));
        let max_calls = self.max_calls;
        let mut functions: BTreeMap<String, BindingFunction> = BTreeMap::new();
        for definition in self.inner.definitions() {
            let (inner, calls, name, parameters) = (
                self.inner.clone(),
                calls.clone(),
                definition.name.clone(),
                definition.parameters,
            );
            let function: BindingFunction = Arc::new(move |args: Value| {
                let (inner, calls, name, parameters) = (
                    inner.clone(),
                    calls.clone(),
                    name.clone(),
                    parameters.clone(),
                );
                Box::pin(async move {
                    if calls.fetch_add(1, Ordering::SeqCst) >= max_calls {
                        return Err(format!("this program has used its {max_calls} tool calls"));
                    }
                    // A program that passes nothing passes no arguments.
                    let args = if args.is_null() { json!({}) } else { args };
                    if let Some(invalid) = crate::validate::invalid_arguments(&parameters, &args) {
                        return Err(invalid);
                    }
                    match inner.execute(&name, &args).await {
                        Some(result) if result.is_error => Err(result.content),
                        Some(result) => Ok(Value::String(result.content)),
                        None => Err(format!("there is no tool `{name}` here")),
                    }
                })
            });
            functions.insert(definition.name, function);
        }
        BindingNamespace {
            global: "tools".to_owned(),
            functions,
            error_class: Some(BindingErrorClass {
                name: "ToolError".to_owned(),
                member_name_property: "tool".to_owned(),
            }),
        }
    }
}

fn cut(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_BYTES {
        let mut at = MAX_OUTPUT_BYTES;
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        text.truncate(at);
        text.push_str("\n[output cut]");
    }
    text
}

fn failed(content: String) -> ToolResult {
    ToolResult {
        content,
        is_error: true,
        details: None,
    }
}

#[async_trait]
impl Tool for RunCode {
    fn name(&self) -> &str {
        NAME
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": NAME,
            "description": format!(
                "Run a short JavaScript program that calls read-only tools itself, so many reads, searches and filters become one call that returns only what you choose. {} Each tool is an async function that returns the tool's text and rejects with a ToolError (its `tool` property names the tool) when the call fails; start several together and `await Promise.all([...])`. Print with console.log; the result must be plain JSON (no undefined, functions, NaN or class instances). There is no file, network or process access beyond these tools; a program gets {} tool calls and {} seconds. Use it when you would otherwise make many similar reads or searches and only need a summary.\nThe tools:\n{}",
                self.runtime.execution_instructions(),
                self.max_calls,
                self.time_limit.as_secs(),
                self.sdk()
            ),
            "parameters": {
                "type": "object",
                "properties": { "code": { "type": "string", "description": "The JavaScript program" } },
                "required": ["code"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(code) = args
            .get("code")
            .and_then(Value::as_str)
            .filter(|c| !c.trim().is_empty())
        else {
            return failed("run_code requires code".into());
        };
        let request = RunRequest {
            program: code.to_owned(),
            state_owner: Some(self.owner),
            bindings: vec![self.bindings()],
            timeout: Timeout::Ms(u64::try_from(self.time_limit.as_millis()).unwrap_or(u64::MAX)),
            ..RunRequest::default()
        };
        let spec = match self.runtime.resolve(request) {
            Ok(spec) => spec,
            Err(message) => return failed(format!("the program could not run: {message}")),
        };
        let result = match self.runtime.run(spec).await {
            Ok(result) => result,
            Err(message) => return failed(format!("the program could not run: {message}")),
        };
        let mut text = result.logs.join("\n");
        if let Some(error) = &result.error {
            if !text.is_empty() {
                text.push('\n');
            }
            text = format!(
                "{text}the program failed ({}): {}",
                error.kind.as_str(),
                error.message.trim_end()
            );
            return failed(cut(text));
        }
        if let Some(value) = &result.value {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("result: ");
            text.push_str(&value.to_string());
        }
        if text.is_empty() {
            "(the program printed and returned nothing)".clone_into(&mut text);
        }
        ToolResult {
            content: cut(text),
            is_error: false,
            details: None,
        }
    }
}

#[cfg(test)]
mod tests;
