//! The vocabulary of program execution: what a caller hands the runtime and what it gets back. Port of the original
//! `ptc-runtime` service definition. A failed program is a field on a resolved result, never an error of `run`:
//! reporting it is the caller's job.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// How program source is evaluated, independent of whether state is kept between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramFormat {
    /// An async function body: top-level `await` and `return` work.
    FunctionBody,
    /// Statements with a final expression, evaluated in one retained global scope; no top-level `return`.
    Repl,
}

/// One host-side function the program calls (and awaits). Arguments and the resolved value are lossless JSON. A
/// rejection (`Err`) surfaces in the program as a rejection of that call.
pub type BindingFunction =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>;

/// A typed rejection for one binding namespace: the runtime injects a real error class under `name`, and a rejected
/// call becomes its instance with the member's name in `member_name_property`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingErrorClass {
    /// The constructor global and the error's `name`.
    pub name: String,
    /// The own property that carries the exact member name.
    pub member_name_property: String,
}

/// A named group of binding functions exposed to the program as one global object (for example `tools`). Function names
/// are arbitrary strings: `__proto__` and `constructor` are ordinary own properties, never prototype collisions.
#[derive(Clone)]
pub struct BindingNamespace {
    /// The global identifier the program sees; see `reserved::validate_namespaces` for what is accepted.
    pub global: String,
    pub functions: BTreeMap<String, BindingFunction>,
    pub error_class: Option<BindingErrorClass>,
}

impl fmt::Debug for BindingNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BindingNamespace")
            .field("global", &self.global)
            .field("functions", &self.functions.keys().collect::<Vec<_>>())
            .field("error_class", &self.error_class)
            .finish()
    }
}

/// An elapsed budget choice. The deadline covers setup and execution, including time waiting for nested calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Timeout {
    /// The provider's default.
    #[default]
    Default,
    /// This many milliseconds, validated and capped by the provider.
    Ms(u64),
    /// No deadline.
    None,
}

/// Identifies who owns retained state. Make one with `Runtime::new_owner` and `release` it when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerId(pub(super) u64);

/// What a caller asks for.
#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    pub program: String,
    /// Retained state is kept for this owner; omitted, a fresh environment is used.
    pub state_owner: Option<OwnerId>,
    pub bindings: Vec<BindingNamespace>,
    /// Working directory; the runtime exposes none to programs, so this only labels the run.
    pub cwd: Option<PathBuf>,
    pub timeout: Timeout,
    /// Stops the program, mid-loop included, and resolves the run as an `abort`.
    pub cancel: Option<CancellationToken>,
}

/// The fully resolved inputs of one run: nothing is filled in later.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub program: String,
    pub state_owner: Option<OwnerId>,
    pub bindings: Vec<BindingNamespace>,
    pub cwd: PathBuf,
    /// After capping; `None` for no deadline.
    pub timeout_ms: Option<u64>,
    pub cancel: Option<CancellationToken>,
}

/// Why a run failed. The kinds are orthogonal outcomes: a budget expiry is not an exception, an abort is not a timeout,
/// and a dead substrate is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The program threw, or failed to parse.
    Exception,
    /// A budget expired; the message says which.
    Timeout,
    /// The caller's token fired.
    Abort,
    /// The execution substrate died without settling.
    WorkerExit,
    /// The completion value was not lossless JSON.
    InvalidOutput,
    /// Logs, value and diagnostic together exceeded the output cap.
    OutputLimit,
    /// The program broke the control protocol (a call over the size or concurrency limits).
    Protocol,
    /// Required confinement could not be set up.
    SandboxUnavailable,
}

impl FailureKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exception => "exception",
            Self::Timeout => "timeout",
            Self::Abort => "abort",
            Self::WorkerExit => "worker-exit",
            Self::InvalidOutput => "invalid-output",
            Self::OutputLimit => "output-limit",
            Self::Protocol => "protocol",
            Self::SandboxUnavailable => "sandbox-unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// Suitable for feeding back to a model so it can correct itself.
    pub message: String,
}

/// The outcome of one run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunResult {
    /// The completion value (a `return`, or the final expression of a REPL program), when it ran to completion and
    /// crossed the lossless-JSON boundary. A failed or value-less run leaves it absent.
    pub value: Option<Value>,
    /// Captured text in emission order.
    pub logs: Vec<String>,
    pub error: Option<Failure>,
}

impl RunResult {
    #[must_use]
    pub fn failed(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            value: None,
            logs: Vec::new(),
            error: Some(Failure {
                kind,
                message: message.into(),
            }),
        }
    }
}
