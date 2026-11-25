//! Program execution: run one model-written program against host async functions and report what it printed and returned.
//! Port of the original `ptc-runtime` (the service definition) and `ptc-runtime-node` (its provider), with an embedded
//! QuickJS engine as the substrate.
//!
//! - [`types`]: what a caller asks for and gets back; a failed program is a field on the result, never an error of `run`.
//! - [`reserved`]: the names every backend refuses, and the check that applies them to a binding list.
//! - [`output`]: exact byte accounting of logs, value and diagnostic against one cap.
//! - `engine`: one environment and the loop that drives a program in it (async bindings, deadlines, cancellation).
//! - [`Runtime`]: resolves requests (defaults and caps), runs them, and keeps retained environments per owner.
//!
//! Two execution modes, as in the original: `PerCall` runs async function bodies, each in a fresh environment; `Persistent`
//! evaluates REPL programs in an environment kept for its owner, so top-level variables survive from one run to the next
//! until the owner is released, the provider is disposed, or a run is stopped from outside. Memory never reaches disk.

#![allow(
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::significant_drop_tightening
)]

mod engine;
pub mod output;
pub mod reserved;
pub mod types;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use tokio::runtime::Handle;

pub use engine::Limits;
pub use types::{
    BindingErrorClass, BindingFunction, BindingNamespace, Failure, FailureKind, OwnerId,
    ProgramFormat, RunRequest, RunResult, RunSpec, Timeout,
};

/// Whether environments are kept between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Async function bodies, a fresh environment for every run.
    PerCall,
    /// REPL programs in an environment kept per owner.
    Persistent,
}

/// The provider's settings; every number must be positive.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub mode: ExecutionMode,
    /// The deadline a request gets when it names none, in milliseconds.
    pub timeout_ms: u64,
    /// The largest deadline a request may ask for.
    pub max_timeout_ms: u64,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::PerCall,
            timeout_ms: 120_000,
            max_timeout_ms: 600_000,
            limits: Limits {
                memory_bytes: 64 * 1024 * 1024,
                stack_bytes: 1024 * 1024,
                max_output_bytes: 4 * 1024 * 1024,
                max_pending_calls: 128,
                max_message_bytes: 8 * 1024 * 1024,
            },
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let l = &self.limits;
        for (name, value) in [
            ("timeout_ms", self.timeout_ms as usize),
            ("max_timeout_ms", self.max_timeout_ms as usize),
            ("memory_bytes", l.memory_bytes),
            ("stack_bytes", l.stack_bytes),
            ("max_output_bytes", l.max_output_bytes),
            ("max_pending_calls", l.max_pending_calls),
            ("max_message_bytes", l.max_message_bytes),
        ] {
            if value == 0 {
                return Err(format!("program runtime: {name} must be positive"));
            }
        }
        if self.timeout_ms > self.max_timeout_ms {
            return Err("program runtime: timeout_ms must not exceed max_timeout_ms".into());
        }
        Ok(())
    }
}

/// A job for an owner's thread.
enum Job {
    Run {
        spec: Box<RunSpec>,
        handle: Handle,
        reply: tokio::sync::oneshot::Sender<RunResult>,
    },
    Stop,
}

/// One owner's retained environment, on a thread of its own because QuickJS is single-threaded.
struct Owner {
    jobs: mpsc::Sender<Job>,
    busy: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// The program runtime.
pub struct Runtime {
    config: Config,
    owners: Mutex<HashMap<OwnerId, Owner>>,
    next_owner: AtomicU64,
    disposed: AtomicBool,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("mode", &self.config.mode)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            config,
            owners: Mutex::new(HashMap::new()),
            next_owner: AtomicU64::new(1),
            disposed: AtomicBool::new(false),
        })
    }

    /// The language programs are written in.
    #[must_use]
    pub fn language(&self) -> &'static str {
        "javascript"
    }

    /// What the programs run on.
    #[must_use]
    pub fn isolation(&self) -> &'static str {
        "thread"
    }

    #[must_use]
    pub fn program_format(&self) -> ProgramFormat {
        match self.config.mode {
            ExecutionMode::PerCall => ProgramFormat::FunctionBody,
            ExecutionMode::Persistent => ProgramFormat::Repl,
        }
    }

    /// Whether memory is kept between runs of one owner.
    #[must_use]
    pub fn retains_state(&self) -> bool {
        self.config.mode == ExecutionMode::Persistent
    }

    /// How programs are to be written in this mode, for the caller to put in front of the model.
    #[must_use]
    pub fn execution_instructions(&self) -> &'static str {
        match self.config.mode {
            ExecutionMode::PerCall => {
                "The program is the body of an async function: use `await` on host calls, `return` the result, and print with console.log. Nothing is kept between runs."
            }
            ExecutionMode::Persistent => {
                "The program is a sequence of statements ending in an expression whose value is the result: top-level `let`, `const`, functions and variables stay for the next run, so a name cannot be declared twice with `let` or `const` (assign to it, or pick a new name). There is no top-level `return`, and `await` only works inside an async function. Memory is kept for this conversation only, never saved, and is cleared if a run times out or is stopped."
            }
        }
    }

    /// The default and the largest deadline a request may have.
    #[must_use]
    pub fn timeout(&self) -> (u64, u64) {
        (self.config.timeout_ms, self.config.max_timeout_ms)
    }

    /// A new owner of retained state; release it with [`release`](Self::release) when it ends.
    #[must_use]
    pub fn new_owner(&self) -> OwnerId {
        OwnerId(self.next_owner.fetch_add(1, Ordering::SeqCst))
    }

    /// Fills in what a request left open and checks what it chose. An error is a request this provider cannot honour.
    pub fn resolve(&self, request: RunRequest) -> Result<RunSpec, String> {
        if self.disposed.load(Ordering::SeqCst) {
            return Err("the program runtime was disposed".into());
        }
        reserved::validate_namespaces(&request.bindings)?;
        let timeout_ms = match request.timeout {
            Timeout::Default => Some(self.config.timeout_ms),
            Timeout::Ms(0) => {
                return Err("timeout must be a positive number of milliseconds".into());
            }
            Timeout::Ms(ms) => Some(ms.min(self.config.max_timeout_ms)),
            Timeout::None => None,
        };
        let cwd = match request.cwd {
            Some(cwd) if cwd.is_absolute() => cwd,
            Some(cwd) => {
                return Err(format!(
                    "the working directory must be absolute: {}",
                    cwd.display()
                ));
            }
            None => std::env::current_dir().map_err(|e| e.to_string())?,
        };
        Ok(RunSpec {
            program: request.program,
            state_owner: request.state_owner,
            bindings: request.bindings,
            cwd,
            timeout_ms,
            cancel: request.cancel,
        })
    }

    /// Runs resolved inputs. How the program ended is in the result; an error is misuse (a disposed runtime, or a second
    /// run for an owner that is still running one).
    pub async fn run(&self, spec: RunSpec) -> Result<RunResult, String> {
        if self.disposed.load(Ordering::SeqCst) {
            return Err("the program runtime was disposed".into());
        }
        let format = self.program_format();
        let handle = Handle::current();
        let limits = self.config.limits;
        let retained = self.retains_state().then_some(spec.state_owner).flatten();
        let Some(owner) = retained else {
            let work = tokio::task::spawn_blocking(move || match engine::Session::new(limits) {
                Ok(mut session) => session.run(&handle, &spec, format).0,
                Err(message) => RunResult::failed(FailureKind::WorkerExit, message),
            });
            return Ok(work.await.unwrap_or_else(|e| {
                RunResult::failed(
                    FailureKind::WorkerExit,
                    format!("the execution thread died: {e}"),
                )
            }));
        };
        let (reply, answer) = tokio::sync::oneshot::channel();
        {
            let mut owners = self.owners.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = owners
                .entry(owner)
                .or_insert_with(|| Self::start_owner(limits, format));
            if entry.busy.swap(true, Ordering::SeqCst) {
                return Err("this owner is already running a program".into());
            }
            if entry
                .jobs
                .send(Job::Run {
                    spec: Box::new(spec),
                    handle,
                    reply,
                })
                .is_err()
            {
                entry.busy.store(false, Ordering::SeqCst);
                owners.remove(&owner);
                return Err("the owner's environment is gone".into());
            }
        }
        let busy = self
            .owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&owner)
            .map(|o| o.busy.clone());
        let result = answer.await.unwrap_or_else(|_| {
            RunResult::failed(
                FailureKind::WorkerExit,
                "the execution thread died before answering",
            )
        });
        if let Some(busy) = busy {
            busy.store(false, Ordering::SeqCst);
        }
        Ok(result)
    }

    fn start_owner(limits: Limits, format: ProgramFormat) -> Owner {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let busy = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn(move || {
            let mut session: Option<engine::Session> = None;
            while let Ok(job) = inbox.recv() {
                match job {
                    Job::Stop => break,
                    Job::Run {
                        spec,
                        handle,
                        reply,
                    } => {
                        if session.is_none() {
                            match engine::Session::new(limits) {
                                Ok(fresh) => session = Some(fresh),
                                Err(message) => {
                                    let _ = reply
                                        .send(RunResult::failed(FailureKind::WorkerExit, message));
                                    continue;
                                }
                            }
                        }
                        let Some(live) = session.as_mut() else {
                            continue;
                        };
                        let (mut result, discard) = live.run(&handle, &spec, format);
                        if discard {
                            // A run stopped from outside leaves the environment in doubt: it starts empty next time,
                            // and the diagnostic says so.
                            session = None;
                            if let Some(error) = result.error.as_mut() {
                                error
                                    .message
                                    .push_str("; the retained variables were cleared");
                            }
                        }
                        let _ = reply.send(result);
                    }
                }
            }
        });
        Owner { jobs, busy, thread }
    }

    /// Ends an owner's environment and waits for its thread. An owner with nothing retained is fine.
    pub async fn release(&self, owner: OwnerId) {
        let found = self
            .owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&owner);
        if let Some(found) = found {
            let _ = found.jobs.send(Job::Stop);
            let _ = tokio::task::spawn_blocking(move || found.thread.join()).await;
        }
    }

    /// Ends every environment; later calls are refused.
    pub async fn dispose(&self) {
        self.disposed.store(true, Ordering::SeqCst);
        let all: Vec<Owner> = self
            .owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, o)| o)
            .collect();
        for owner in all {
            let _ = owner.jobs.send(Job::Stop);
            let _ = tokio::task::spawn_blocking(move || owner.thread.join()).await;
        }
    }
}

#[cfg(test)]
mod tests;
