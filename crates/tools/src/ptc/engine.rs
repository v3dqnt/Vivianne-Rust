//! One QuickJS environment and the loop that drives a program in it. Binding calls are real promises: the program
//! `await`s them, several that it starts together are awaited together, and the host settles each one when its work is
//! done. A deadline, the caller's token, and the output cap all stop the program mid-loop.
//!
//! A `Session` is one environment. A fresh run builds one and throws it away; a retained owner keeps its `Session`
//! (and so its top-level variables) between runs. Everything here is blocking and single-threaded, as QuickJS is: it runs
//! on a thread of its own.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use rquickjs::{Context, Ctx, Function, Persistent, Promise, Runtime, Value as JsValue};
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use super::output::OutputLedger;
use super::types::{
    BindingFunction, BindingNamespace, Failure, FailureKind, ProgramFormat, RunResult, RunSpec,
};

const PRELUDE: &str = include_str!("prelude.js");

/// What the environment may use.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub max_output_bytes: usize,
    pub max_pending_calls: usize,
    /// Most bytes of one call's serialized arguments.
    pub max_message_bytes: usize,
}

/// What the interrupt handler reads, set for each run.
#[derive(Default)]
struct Control {
    deadline: Mutex<Option<Instant>>,
    cancel: Mutex<Option<CancellationToken>>,
    /// Set when the output cap or a protocol limit ends the run.
    stop: AtomicBool,
}

impl Control {
    fn timed_out(&self) -> bool {
        self.deadline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|d| Instant::now() >= d)
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }
}

/// A call the program made that the host has not answered yet.
struct PendingCall {
    namespace: usize,
    member: String,
    args: Value,
    resolve: Persistent<Function<'static>>,
    reject: Persistent<Function<'static>>,
}

/// What the host functions share with the loop: one run's books.
struct HostState {
    namespaces: Vec<BindingNamespace>,
    pending: Vec<PendingCall>,
    logs: Vec<String>,
    ledger: OutputLedger,
    /// The output cap was crossed.
    over_limit: bool,
    /// The program broke a protocol limit; this is what to tell it.
    protocol: Option<String>,
    limits: Limits,
}

impl HostState {
    fn fresh(limits: Limits) -> Self {
        Self {
            namespaces: Vec::new(),
            pending: Vec::new(),
            logs: Vec::new(),
            ledger: OutputLedger::new(limits.max_output_bytes),
            over_limit: false,
            protocol: None,
            limits,
        }
    }
}

/// One environment: a QuickJS runtime and context with the prelude installed.
///
/// The fields are in the order they must be dropped: everything that holds a JavaScript value first, the runtime last. A
/// value freed after its runtime would trip the engine's own leak check.
pub struct Session {
    describe: Persistent<Function<'static>>,
    snapshot: Persistent<Function<'static>>,
    reject_with: Persistent<Function<'static>>,
    install: Persistent<Function<'static>>,
    complete: Persistent<Function<'static>>,
    state: Rc<RefCell<HostState>>,
    control: Arc<Control>,
    limits: Limits,
    context: Context,
    runtime: Runtime,
}

/// Programs run in sloppy mode, as a REPL does: assigning to an undeclared name makes a retained global.
fn sloppy() -> rquickjs::context::EvalOptions {
    let mut options = rquickjs::context::EvalOptions::default();
    options.strict = false;
    options
}

fn js_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// The host function behind every binding call: it returns a promise and queues the call for the loop to answer.
fn make_invoke<'js>(
    ctx: &Ctx<'js>,
    host: Rc<RefCell<HostState>>,
    watch: Arc<Control>,
) -> rquickjs::Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>,
              namespace: u32,
              member: String,
              args: String|
              -> rquickjs::Result<Promise<'js>> {
            let (promise, resolve, reject) = Promise::new(&ctx)?;
            let mut host = host.borrow_mut();
            if host.pending.len() >= host.limits.max_pending_calls {
                host.protocol = Some(format!(
                    "the program started more than {} host calls at once",
                    host.limits.max_pending_calls
                ));
                watch.stop.store(true, Ordering::SeqCst);
            } else if args.len() > host.limits.max_message_bytes {
                host.protocol = Some(format!(
                    "a call's arguments were larger than {} bytes",
                    host.limits.max_message_bytes
                ));
                watch.stop.store(true, Ordering::SeqCst);
            } else {
                let args = serde_json::from_str(&args).unwrap_or(Value::Null);
                host.pending.push(PendingCall {
                    namespace: namespace as usize,
                    member,
                    args,
                    resolve: Persistent::save(&ctx, resolve),
                    reject: Persistent::save(&ctx, reject),
                });
            }
            Ok(promise)
        },
    )
}

impl Session {
    pub fn new(limits: Limits) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(js_error)?;
        runtime.set_memory_limit(limits.memory_bytes);
        runtime.set_max_stack_size(limits.stack_bytes);
        let control = Arc::new(Control::default());
        let watched = control.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            watched.stop.load(Ordering::SeqCst) || watched.timed_out() || watched.cancelled()
        })));
        let context = Context::full(&runtime).map_err(js_error)?;
        let state = Rc::new(RefCell::new(HostState::fresh(limits)));
        let (describe, snapshot, reject_with, install, complete) =
            context.with(|ctx| -> Result<_, String> {
                let globals = ctx.globals();
                let host = state.clone();
                let log = Function::new(ctx.clone(), move |text: String| {
                    let mut host = host.borrow_mut();
                    if host.over_limit {
                        return;
                    }
                    let HostState { ledger, logs, .. } = &mut *host;
                    if !ledger.admit_prefix(&text, logs) {
                        host.over_limit = true;
                    }
                })
                .map_err(js_error)?;
                globals.set("__log", log).map_err(js_error)?;
                let host = state.clone();
                let watch = control.clone();
                let invoke = make_invoke(&ctx, host, watch).map_err(js_error)?;
                globals.set("__invoke", invoke).map_err(js_error)?;
                ctx.eval::<(), _>(PRELUDE).map_err(js_error)?;
                let grab = |name: &str| -> Result<Persistent<Function<'static>>, String> {
                    let function: Function<'_> = globals.get(name).map_err(js_error)?;
                    Ok(Persistent::save(&ctx, function))
                };
                Ok((
                    grab("__describe")?,
                    grab("__snapshot")?,
                    grab("__reject_with")?,
                    grab("__install")?,
                    grab("__complete")?,
                ))
            })?;
        Ok(Self {
            describe,
            snapshot,
            reject_with,
            install,
            complete,
            state,
            control,
            limits,
            context,
            runtime,
        })
    }

    /// What the program threw, as text a model can act on.
    fn describe<'js>(&self, ctx: &Ctx<'js>, thrown: JsValue<'js>) -> String {
        let shown = self
            .describe
            .clone()
            .restore(ctx)
            .and_then(|f| f.call::<_, String>((thrown,)));
        shown.unwrap_or_else(|_| "the program threw a value that cannot be shown".to_owned())
    }

    fn describe_error(&self, ctx: &Ctx<'_>, error: &rquickjs::Error) -> String {
        match error {
            rquickjs::Error::Exception => self.describe(ctx, ctx.catch()),
            other => other.to_string(),
        }
    }

    /// Runs one program to its end. `retained` says the environment lives on, so a failure that leaves it in doubt
    /// (a stop in the middle of the program) says its variables are cleared.
    pub fn run(
        &mut self,
        handle: &Handle,
        spec: &RunSpec,
        format: ProgramFormat,
    ) -> (RunResult, bool) {
        let started = Instant::now();
        let deadline = spec
            .timeout_ms
            .map(|ms| started + std::time::Duration::from_millis(ms));
        *self
            .control
            .deadline
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = deadline;
        self.control
            .cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&spec.cancel);
        self.control.stop.store(false, Ordering::SeqCst);
        *self.state.borrow_mut() = HostState {
            namespaces: spec.bindings.clone(),
            ..HostState::fresh(self.limits)
        };

        let launched = self.launch(spec, format);
        let mut code_started = launched.is_ok() || matches!(launched, Err(Launch::Threw(_)));
        let mut thrown = launched.err();
        if thrown.is_none() {
            thrown = self.drive(handle, spec).err();
            code_started = true;
        }
        let verdict = self.verdict(spec, code_started);
        let (result, discard) = match verdict {
            Some((failure, discard)) => {
                let state = self.state.borrow();
                (
                    state.ledger.failure(state.logs.clone(), failure.clone()),
                    discard,
                )
            }
            None => match thrown {
                Some(Launch::Threw(message)) => {
                    let state = self.state.borrow();
                    (
                        state.ledger.failure(
                            state.logs.clone(),
                            Failure {
                                kind: FailureKind::Exception,
                                message,
                            },
                        ),
                        false,
                    )
                }
                Some(Launch::Failed(failure)) => {
                    let state = self.state.borrow();
                    (state.ledger.failure(state.logs.clone(), failure), true)
                }
                None => (self.finish(), false),
            },
        };
        // The output cap is its own outcome: it keeps the logs that fit.
        let result = if self.state.borrow().over_limit {
            self.state
                .borrow()
                .ledger
                .limit(self.state.borrow().logs.clone())
        } else {
            result
        };
        self.state.borrow_mut().pending.clear();
        (result, discard || self.state.borrow().over_limit)
    }

    /// Timeouts, aborts, protocol breaks: the outcomes that end a program from outside. The flag says the
    /// environment cannot be trusted afterwards.
    fn verdict(&self, spec: &RunSpec, code_started: bool) -> Option<(Failure, bool)> {
        if let Some(message) = self.state.borrow().protocol.clone() {
            return Some((
                Failure {
                    kind: FailureKind::Protocol,
                    message,
                },
                true,
            ));
        }
        if self.control.cancelled() {
            return Some((
                Failure {
                    kind: FailureKind::Abort,
                    message: "execution was cancelled".to_owned(),
                },
                true,
            ));
        }
        if self.control.timed_out() {
            let budget = spec.timeout_ms.unwrap_or(0);
            let message = if code_started {
                format!("execution timed out during execution (budget {budget} ms)")
            } else {
                format!("execution timed out before code started (budget {budget} ms)")
            };
            return Some((
                Failure {
                    kind: FailureKind::Timeout,
                    message,
                },
                true,
            ));
        }
        None
    }

    /// Installs the bindings and starts the program, up to its first wait.
    fn launch(&self, spec: &RunSpec, format: ProgramFormat) -> Result<(), Launch> {
        let described: Vec<Value> = spec
            .bindings
            .iter()
            .map(|n| {
                json!({
                    "global": n.global,
                    "functions": n.functions.keys().collect::<Vec<_>>(),
                    "errorClass": n.error_class.as_ref().map(|c| json!({ "name": c.name, "property": c.member_name_property })),
                })
            })
            .collect();
        let spec_json = Value::Array(described).to_string();
        self.context.with(|ctx| {
            let install = self.install.clone().restore(&ctx).map_err(|e| Launch::Threw(js_error(e)))?;
            install.call::<_, ()>((spec_json,)).map_err(|e| Launch::Threw(self.describe_error(&ctx, &e)))?;
            let outcome = match format {
                ProgramFormat::FunctionBody => {
                    // The body runs as an async function so `await` and `return` work. Its settlement is recorded
                    // where the loop can read it, out of the program's reach by name only (it can still look).
                    let wrapped = format!(
                        "globalThis.__done = undefined; (async function () {{\n{}\n}})().then((v) => {{ globalThis.__done = {{ ok: v }}; }}, (e) => {{ globalThis.__done = {{ err: e }}; }});",
                        spec.program
                    );
                    ctx.eval_with_options::<(), _>(wrapped, sloppy())
                }
                ProgramFormat::Repl => ctx.eval_with_options::<JsValue, _>(spec.program.as_str(), sloppy()).and_then(|completion| {
                    let complete = self.complete.clone().restore(&ctx)?;
                    complete.call::<_, ()>((completion,))
                }),
            };
            outcome.map_err(|e| match e {
                rquickjs::Error::Exception => Launch::Threw(self.describe(&ctx, ctx.catch())),
                other => Launch::Threw(other.to_string()),
            })
        })
    }

    /// Runs the program's jobs, and the host calls it makes, until it settles or is stopped.
    fn drive(&self, handle: &Handle, spec: &RunSpec) -> Result<(), Launch> {
        loop {
            while self.runtime.is_job_pending() {
                if self.control.stop.load(Ordering::SeqCst)
                    || self.control.timed_out()
                    || self.control.cancelled()
                {
                    return Ok(());
                }
                if let Err(exception) = self.runtime.execute_pending_job() {
                    let message = exception.0.with(|ctx| self.describe(&ctx, ctx.catch()));
                    return Err(Launch::Threw(message));
                }
            }
            if self.control.stop.load(Ordering::SeqCst)
                || self.control.timed_out()
                || self.control.cancelled()
            {
                return Ok(());
            }
            if self.settled() {
                return Ok(());
            }
            let calls = std::mem::take(&mut self.state.borrow_mut().pending);
            if calls.is_empty() {
                return Err(Launch::Threw(
                    "the program ended with a promise that can never settle".to_owned(),
                ));
            }
            self.settle(handle, spec, calls);
        }
    }

    fn settled(&self) -> bool {
        self.context.with(|ctx| {
            ctx.globals()
                .get::<_, JsValue>("__done")
                .is_ok_and(|v| !v.is_undefined())
        })
    }

    /// Answers every call the program made, together, and hands the answers back to it.
    fn settle(&self, handle: &Handle, spec: &RunSpec, calls: Vec<PendingCall>) {
        let functions: Vec<Option<BindingFunction>> = {
            let state = self.state.borrow();
            calls
                .iter()
                .map(|c| {
                    state
                        .namespaces
                        .get(c.namespace)
                        .and_then(|n| n.functions.get(&c.member))
                        .cloned()
                })
                .collect()
        };
        let futures = calls.iter().zip(&functions).map(|(call, function)| {
            let args = call.args.clone();
            let function = function.clone();
            async move {
                match function {
                    Some(function) => function(args).await,
                    None => Err("that function is not provided".to_owned()),
                }
            }
        });
        let (cancel, deadline) = (
            spec.cancel.clone(),
            self.control
                .deadline
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .copied(),
        );
        let answers = handle.block_on(async {
            let wait = async {
                match (&cancel, deadline) {
                    (Some(token), Some(at)) => tokio::select! { () = token.cancelled() => (), () = tokio::time::sleep_until(tokio::time::Instant::from_std(at)) => () },
                    (Some(token), None) => token.cancelled().await,
                    (None, Some(at)) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
                    (None, None) => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                answers = futures::future::join_all(futures) => Some(answers),
                () = wait => None,
            }
        });
        let Some(answers) = answers else { return };
        self.context.with(|ctx| {
            let Ok(reject_with) = self.reject_with.clone().restore(&ctx) else {
                return;
            };
            for (call, answer) in calls.into_iter().zip(answers) {
                match answer {
                    Ok(value) => {
                        let Ok(resolve) = call.resolve.restore(&ctx) else {
                            continue;
                        };
                        let parsed = ctx.json_parse(value.to_string()).ok();
                        match parsed {
                            Some(parsed) => {
                                let _ = resolve.call::<_, ()>((parsed,));
                            }
                            None => {
                                let _ =
                                    resolve.call::<_, ()>((JsValue::new_undefined(ctx.clone()),));
                            }
                        }
                    }
                    Err(message) => {
                        let Ok(reject) = call.reject.restore(&ctx) else {
                            continue;
                        };
                        if let Ok(error) = reject_with.call::<_, JsValue>((
                            call.namespace as u32,
                            call.member.clone(),
                            message,
                        )) {
                            let _ = reject.call::<_, ()>((error,));
                        }
                    }
                }
            }
        });
    }

    /// The program ended on its own: its value, or what it threw.
    fn finish(&self) -> RunResult {
        let outcome = self.context.with(|ctx| -> Result<Option<String>, Launch> {
            let done: JsValue = ctx
                .globals()
                .get("__done")
                .map_err(|e| Launch::Threw(js_error(e)))?;
            let Some(done) = done.as_object() else {
                return Err(Launch::Threw("the program did not finish".to_owned()));
            };
            if done.contains_key("err").unwrap_or(false) {
                let error: JsValue = done.get("err").map_err(|e| Launch::Threw(js_error(e)))?;
                return Err(Launch::Threw(self.describe(&ctx, error)));
            }
            let value: JsValue = done.get("ok").map_err(|e| Launch::Threw(js_error(e)))?;
            if value.is_undefined() {
                return Ok(None);
            }
            let snapshot = self
                .snapshot
                .clone()
                .restore(&ctx)
                .map_err(|e| Launch::Threw(js_error(e)))?;
            match snapshot.call::<_, String>((value, "the completion value")) {
                Ok(text) => Ok(Some(text)),
                Err(error) => Err(Launch::Failed(Failure {
                    kind: FailureKind::InvalidOutput,
                    message: self.describe_error(&ctx, &error),
                })),
            }
        });
        let state = self.state.borrow();
        match outcome {
            Ok(None) => state.ledger.success(state.logs.clone(), None),
            Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => state.ledger.success(state.logs.clone(), Some(value)),
                Err(error) => state.ledger.failure(
                    state.logs.clone(),
                    Failure {
                        kind: FailureKind::InvalidOutput,
                        message: error.to_string(),
                    },
                ),
            },
            Err(Launch::Threw(message)) => state.ledger.failure(
                state.logs.clone(),
                Failure {
                    kind: FailureKind::Exception,
                    message,
                },
            ),
            Err(Launch::Failed(failure)) => state.ledger.failure(state.logs.clone(), failure),
        }
    }
}

/// Why starting or driving a program ended it before a value.
enum Launch {
    /// The program threw or failed to parse.
    Threw(String),
    Failed(Failure),
}
