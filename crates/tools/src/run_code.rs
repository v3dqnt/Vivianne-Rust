//! `run_code`: the model writes a small JavaScript program that calls the read-only tools itself, loops, filters and
//! combines what they return, and answers with only the result. Many reads become one call whose output is what the
//! program chose to say, so the conversation carries the answer and not every file the program looked at. Port of the
//! original programmatic tool executor (`tool-executor-ptc`), narrowed to what is safe here.
//!
//! The program runs in an embedded `QuickJS` engine with no file, network, process or module access of its own: the only
//! way out is the generated `tools` object, and that holds only tools that read (`read`, `ls`, `grep`, `code_search`,
//! `glob`), so `run_code` itself only looks and needs no approval. A program has a time limit, a memory limit, a stack
//! limit and a limit on how many tool calls it may make. Calls are validated against each tool's schema like the
//! model's own, and `tools.all([...])` runs several at once.

#![allow(clippy::needless_pass_by_value, clippy::duration_suboptimal_units, clippy::map_unwrap_or, clippy::doc_markdown)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rquickjs::{Context, Function, Runtime};
use serde_json::{Value, json};
use tokio::runtime::Handle;

use crate::{Tool, ToolResult, Toolbox, builtin};

/// The tool's name.
pub const NAME: &str = "run_code";

/// Wall time of one program, tool calls included.
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// Memory the engine may use.
pub const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// Stack the engine may use.
pub const STACK_LIMIT: usize = 1024 * 1024;
/// Tool calls one program may make.
pub const MAX_CALLS: usize = 50;
/// Calls of one `tools.all` that run at the same time.
pub const MAX_PARALLEL: usize = 10;
/// Most bytes of what a program prints and returns.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// The JavaScript that builds the `tools` object and `console` from the two host functions.
const PRELUDE: &str = r"
globalThis.__out = [];
const __show = (v) => typeof v === 'string' ? v : (v === undefined ? 'undefined' : JSON.stringify(v));
globalThis.console = {
  log: (...a) => { __out.push(a.map(__show).join(' ')); },
  info: (...a) => { __out.push(a.map(__show).join(' ')); },
  warn: (...a) => { __out.push(a.map(__show).join(' ')); },
  error: (...a) => { __out.push(a.map(__show).join(' ')); },
};
const __unwrap = (r) => { if (r.error !== undefined) throw new Error(r.error); return r.ok; };
globalThis.tools = {};
for (const name of __names) {
  tools[name] = (args) => __unwrap(JSON.parse(__call(name, JSON.stringify(args === undefined ? {} : args))));
}
tools.all = (calls) => {
  const settled = JSON.parse(__all(JSON.stringify(calls.map((c) => [c[0], c[1] === undefined ? {} : c[1]]))));
  return settled.map(__unwrap);
};
";

/// Runs programs that call read-only tools.
pub struct RunCode {
    inner: Arc<Toolbox>,
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
    /// Programs that read in `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        let cwd = root.to_string_lossy().into_owned();
        let mut inner = Toolbox::new(root);
        inner.add(builtin::ReadTool::new(cwd.clone()));
        inner.add(builtin::LsTool::new(cwd.clone()));
        inner.add(builtin::GrepTool::new(cwd.clone()));
        inner.add(builtin::CodeSearchTool::new(cwd.clone()));
        inner.add(builtin::GlobTool::new(cwd));
        Self {
            inner: Arc::new(inner),
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
                format!("tools.{}({{ {} }})", d.name, args.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// One host call, as the program sees its answer: `{"ok": text}` or `{"error": text}`.
fn answer(result: Result<String, String>) -> String {
    match result {
        Ok(text) => json!({ "ok": text }),
        Err(text) => json!({ "error": text }),
    }
    .to_string()
}

/// Runs one tool for a program: counted against its limit, checked against the schema, answered as text.
async fn call_one(
    inner: &Toolbox,
    calls: &AtomicUsize,
    max_calls: usize,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    if calls.fetch_add(1, Ordering::SeqCst) >= max_calls {
        return Err(format!("this program has used its {max_calls} tool calls"));
    }
    let definition = inner.definitions().into_iter().find(|d| d.name == name).ok_or_else(|| format!("there is no tool `{name}` here; the ones a program can call are listed in the run_code description"))?;
    if let Some(invalid) = crate::validate::invalid_arguments(&definition.parameters, args) {
        return Err(invalid);
    }
    match inner.execute(name, args).await {
        Some(result) if result.is_error => Err(result.content),
        Some(result) => Ok(result.content),
        None => Err(format!("there is no tool `{name}` here")),
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

/// Runs `code` as the body of a function and gives back what it printed and returned. Blocking: call it from a thread
/// meant for that.
fn run(
    inner: &Arc<Toolbox>,
    handle: &Handle,
    code: &str,
    time_limit: Duration,
    max_calls: usize,
) -> Result<String, String> {
    let engine = Runtime::new().map_err(|e| e.to_string())?;
    engine.set_memory_limit(MEMORY_LIMIT);
    engine.set_max_stack_size(STACK_LIMIT);
    let deadline = Instant::now() + time_limit;
    engine.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&engine).map_err(|e| e.to_string())?;
    let calls = Arc::new(AtomicUsize::new(0));
    let names: Vec<String> = inner.definitions().into_iter().map(|d| d.name).collect();
    context.with(|ctx| -> Result<String, String> {
        let describe = |error: rquickjs::Error| match error {
            rquickjs::Error::Exception => {
                let thrown = ctx.catch();
                thrown
                    .as_exception()
                    .map(|e| {
                        let message = e.message().unwrap_or_default();
                        match e.stack() {
                            Some(stack) if !stack.is_empty() => format!("{message}\n{stack}"),
                            _ => message,
                        }
                    })
                    .unwrap_or_else(|| "the program threw a value that is not an Error".to_owned())
            }
            other if Instant::now() >= deadline => format!("the program ran out of time ({other})"),
            other => other.to_string(),
        };
        let globals = ctx.globals();
        {
            let (inner, calls, handle) = (inner.clone(), calls.clone(), handle.clone());
            let one = Function::new(ctx.clone(), move |name: String, args: String| -> String {
                let result = match serde_json::from_str::<Value>(&args) {
                    Ok(args) => handle.block_on(call_one(&inner, &calls, max_calls, &name, &args)),
                    Err(error) => Err(format!("the arguments are not JSON: {error}")),
                };
                answer(result)
            })
            .map_err(&describe)?;
            globals.set("__call", one).map_err(&describe)?;
        }
        {
            let (inner, calls, handle) = (inner.clone(), calls.clone(), handle.clone());
            let many = Function::new(ctx.clone(), move |batch: String| -> String {
                let Ok(list) = serde_json::from_str::<Vec<(String, Value)>>(&batch) else {
                    return Value::Array(vec![json!({ "error": "tools.all takes a list of [name, arguments] pairs" })]).to_string();
                };
                let mut settled: Vec<Value> = Vec::new();
                for group in list.chunks(MAX_PARALLEL) {
                    let results = handle.block_on(futures::future::join_all(group.iter().map(|(name, args)| call_one(&inner, &calls, max_calls, name, args))));
                    settled.extend(results.into_iter().map(|r| serde_json::from_str(&answer(r)).unwrap_or(Value::Null)));
                }
                Value::Array(settled).to_string()
            })
            .map_err(&describe)?;
            globals.set("__all", many).map_err(&describe)?;
        }
        globals.set("__names", serde_json::to_string(&names).map_err(|e| e.to_string())?).map_err(&describe)?;
        ctx.eval::<(), _>("globalThis.__names = JSON.parse(__names);").map_err(&describe)?;
        ctx.eval::<(), _>(PRELUDE).map_err(&describe)?;
        let program = format!("(function () {{ const r = (function () {{\n{code}\n}})(); return r === undefined ? '' : JSON.stringify(r); }})()");
        let returned: String = ctx.eval(program).map_err(&describe)?;
        let printed: Vec<String> = ctx.eval("__out").map_err(&describe)?;
        let mut text = printed.join("\n");
        if !returned.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("result: ");
            text.push_str(&returned);
        }
        Ok(text)
    })
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
                "Run a short JavaScript program that calls read-only tools itself, so many reads, searches and filters become one call that returns only what you choose. The code is the body of a function: call `tools.<name>({{...}})` (each returns the tool's text, or throws an Error you can catch), use `tools.all([[name, args], ...])` to run several at once, print with console.log, and `return` a value (JSON) as the result. There is no file, network or process access beyond these tools; a program gets {} tool calls and {} seconds. Use it when you would otherwise make many similar reads or searches and only need a summary.\nThe tools:\n{}",
                self.max_calls,
                self.time_limit.as_secs(),
                self.sdk()
            ),
            "parameters": {
                "type": "object",
                "properties": { "code": { "type": "string", "description": "The body of a JavaScript function" } },
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
            return ToolResult {
                content: "run_code requires code".into(),
                is_error: true,
                details: None,
            };
        };
        let (inner, code, handle) = (self.inner.clone(), code.to_owned(), Handle::current());
        let (time_limit, max_calls) = (self.time_limit, self.max_calls);
        let work =
            tokio::task::spawn_blocking(move || run(&inner, &handle, &code, time_limit, max_calls));
        match work.await {
            Ok(Ok(text)) if text.is_empty() => ToolResult {
                content: "(the program printed and returned nothing)".into(),
                is_error: false,
                details: None,
            },
            Ok(Ok(text)) => ToolResult {
                content: cut(text),
                is_error: false,
                details: None,
            },
            Ok(Err(message)) => ToolResult {
                content: cut(format!("the program failed: {message}")),
                is_error: true,
                details: None,
            },
            Err(error) => ToolResult {
                content: format!("the program could not run: {error}"),
                is_error: true,
                details: None,
            },
        }
    }
}

#[cfg(test)]
mod tests;
