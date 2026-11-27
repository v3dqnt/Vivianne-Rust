//! The background-job contract: statuses, channels, projections and the producer face.
//!
//! Port of `@deepseek-ai/dsh-jobs` (`types.ts`, `view.ts`). Job ids are plain strings shaped
//! `<kind>-<16 lowercase uuid hex>`; ownership is a thread id, and access is fenced by it.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A job's identity: `<kind>-<16 lowercase uuid hex>`.
pub type JobId = String;

/// A thread (session) id, used to fence who may see and control a job.
pub type SessionId = String;

/// Job lifecycle: `running`, optionally `stopping`, then exactly one terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Killed,
    Failed,
}

impl JobStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Killed => "killed",
            Self::Failed => "failed",
        }
    }

    /// Whether the job has reached its one terminal status.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Killed | Self::Failed)
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stream label of one output chunk. `log` is producer narration only observers see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobChannel {
    Stdout,
    Stderr,
    Log,
}

impl JobChannel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Log => "log",
        }
    }
}

impl fmt::Display for JobChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Terminal result supplied by a producer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutcome {
    /// How the job ended.
    pub status: JobStatus,
    /// Terminal reason rendered into status lines (`exit code: 3`).
    pub detail: Option<String>,
    /// Return value for jobs whose result is a value rather than a stream, handed out once.
    pub result: Option<String>,
}

impl JobOutcome {
    #[must_use]
    pub fn completed() -> Self {
        Self {
            status: JobStatus::Completed,
            detail: None,
            result: None,
        }
    }

    #[must_use]
    pub fn failed(detail: impl Into<String>) -> Self {
        Self {
            status: JobStatus::Failed,
            detail: Some(detail.into()),
            result: None,
        }
    }
}

/// One chunk of a job's output ring: absolute offset, text, channel, and loss marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobChunk {
    /// Absolute offset of the chunk's first byte; offsets never move once assigned.
    pub at: u64,
    /// Chunk text exactly as appended (possibly tail-trimmed by retention).
    pub text: String,
    /// Stream label, when the producer supplied one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<JobChannel>,
    /// Bytes immediately before this chunk were lost.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gap_before: bool,
}

/// Options for one append to a job's ring.
#[derive(Debug, Clone, Default)]
pub struct JobAppendOptions {
    /// Stream label for the chunk; omitted for producers without distinct streams.
    pub channel: Option<JobChannel>,
    /// Producer-side bytes between the previous chunk and this one were lost.
    pub gap_before: bool,
}

/// One incremental read from a [`JobOutputSource`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobSourceRead {
    /// Text captured since the requested offset (the whole retained tail when lossy).
    pub text: String,
    /// Whole-stream offset to resume from on the next read.
    pub next_offset: u64,
    /// True when the requested offset slid out of the source's retained window.
    pub lossy: bool,
    /// Host path of a file holding the complete stream, when the source keeps one.
    pub spill_path: Option<String>,
}

/// A pull source the registry pumps into a job's ring.
pub trait JobOutputSource: Send + Sync {
    /// Stream label attached to every chunk this source yields.
    fn channel(&self) -> Option<JobChannel> {
        None
    }

    /// Read everything captured since `from_byte` without consuming it.
    fn read(&self, from_byte: u64) -> JobSourceRead;
}

/// The output ring's absolute coordinates and the complete-stream files behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobOutputView {
    /// The offset the next chunk starts at.
    pub total: u64,
    /// The oldest retained byte; greater than zero exactly when retention dropped the head.
    pub earliest: u64,
    /// Spill files the job's pull sources currently keep, in source order and deduplicated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spill_paths: Option<Vec<String>>,
}

/// A fresh read-only projection of one job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobView {
    pub id: JobId,
    pub kind: String,
    pub label: String,
    /// Owning session; absent for an unowned job, which every caller can see.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<SessionId>,
    /// Producer-owned cap for complete model-facing notices and reads, in UTF-8 bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_limit_bytes: Option<usize>,
    pub status: JobStatus,
    /// The producer's live progress line; cleared at settlement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Terminal reason (`exit code: 3`); a recorded kill reason is merged in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    pub output: JobOutputView,
}

/// Output and post-read state returned by the consuming [`crate::jobs::Registry::read`].
#[derive(Debug, Clone)]
pub struct JobRead {
    /// Ring chunks appended since the model cursor, in offset order; every channel included.
    pub chunks: Vec<JobChunk>,
    /// True when the cursor fell below the oldest retained byte.
    pub lossy: bool,
    /// The producer's result, handed out by the first read after settlement only.
    pub result: Option<String>,
    /// The job's state at read time.
    pub job: JobView,
}

/// Result of one non-consuming [`crate::jobs::Registry::read_at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutputRead {
    /// Retained chunks overlapping `[from, total)`, in offset order.
    pub chunks: Vec<JobChunk>,
    /// Offset to resume from — the ring's current `total`.
    pub next: u64,
    /// True when `from` fell below the oldest retained byte.
    pub lossy: bool,
}

/// Why a job settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSettleCause {
    /// Its producer finished.
    Producer,
    /// A kill ran first.
    Kill,
    /// Owner or service teardown cancelled it.
    Teardown,
}

impl JobSettleCause {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Producer => "producer",
            Self::Kill => "kill",
            Self::Teardown => "teardown",
        }
    }
}

/// One lifecycle or output event.
#[derive(Debug, Clone)]
pub enum JobEvent {
    /// Registration commit.
    Registered(JobView),
    /// Progress line change.
    Progress(JobView),
    /// Stopping transition.
    Stopping(JobView),
    /// Removal from the visible set.
    Removed(JobView),
    /// Terminal record.
    Settled {
        job: JobView,
        cause: JobSettleCause,
        /// Whether this settlement released a live wait.
        awaited: bool,
    },
    /// The ring advanced (or settled, which ends the stream).
    Output {
        id: JobId,
        owner: Option<SessionId>,
        total: u64,
    },
}

impl JobEvent {
    /// The event's type name, for logs.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Registered(_) => "registered",
            Self::Progress(_) => "progress",
            Self::Stopping(_) => "stopping",
            Self::Removed(_) => "removed",
            Self::Settled { .. } => "settled",
            Self::Output { .. } => "output",
        }
    }

    /// The owning session, absent for an unowned job.
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        match self {
            Self::Registered(job)
            | Self::Progress(job)
            | Self::Stopping(job)
            | Self::Removed(job)
            | Self::Settled { job, .. } => job.owner.as_deref(),
            Self::Output { owner, .. } => owner.as_deref(),
        }
    }
}

/// Who a subscription hears about.
///
/// The TypeScript contract has three forms (`{ owner }`, `{ owners: 'scope' }`, `{ owners: 'all' }`);
/// one process-wide registry with one plugin instance collapses them to "one owner" or "everything".
#[derive(Debug, Clone, Default)]
pub enum JobEventFilter {
    /// Only this session's jobs plus every unowned job.
    Owner(SessionId),
    /// Everything.
    #[default]
    All,
}

/// Event callback; contained by the registry, never awaited.
pub type JobEventListener = Arc<dyn Fn(&JobEvent) + Send + Sync>;

/// The producer face of one registered job.
#[derive(Clone)]
pub struct JobHandle {
    pub(crate) id: JobId,
    pub(crate) job: std::sync::Weak<super::job::TrackedJob>,
}

impl JobHandle {
    /// The registry-issued id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Append one chunk to the output ring. An empty chunk is dropped.
    pub fn append(&self, text: impl AsRef<str>, options: JobAppendOptions) {
        if let Some(job) = self.job.upgrade() {
            job.append(text.as_ref(), options, Writer::Producer);
        }
    }

    /// Replace the live progress line.
    pub fn update_progress(&self, line: impl Into<String>) {
        if let Some(job) = self.job.upgrade() {
            job.update_progress(line.into());
        }
    }
}

impl fmt::Debug for JobHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobHandle").field("id", &self.id).finish()
    }
}

/// Asks a producer to stop, with why.
pub type JobCancel = Arc<dyn Fn(Option<&str>) + Send + Sync>;

/// Hooks through which the runtime controls and observes producer work.
pub struct JobHooks {
    /// Resolves after the producer releases its resources. A rejection is converted to `failed`.
    pub done: futures::future::BoxFuture<'static, JobOutcome>,
    /// Request termination; synchronous, idempotent, and eventually settles [`done`](Self::done).
    pub cancel: JobCancel,
}

impl fmt::Debug for JobHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JobHooks")
    }
}

/// Producer declaration passed to [`crate::jobs::Registry::start`].
pub struct JobSpec {
    /// Producer kind — also the id prefix (`bash`, …).
    pub kind: String,
    /// One-line model-facing label (the command; the delegation description).
    pub label: String,
    /// Owning session. Its live agent's disposal cancels and awaits the job.
    pub owner: Option<SessionId>,
    /// Optional UTF-8 byte cap for each complete model-facing completion notice or output read.
    pub output_limit_bytes: Option<usize>,
    /// Pull sources the registry pumps into the ring at its own cadence.
    pub output: Vec<Box<dyn JobOutputSource>>,
    /// Start the work after preflight and synchronously return its hooks.
    pub run: Box<dyn FnOnce(JobHandle) -> JobHooks + Send>,
}

impl fmt::Debug for JobSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobSpec")
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("owner", &self.owner)
            .field("output", &self.output.len())
            .finish_non_exhaustive()
    }
}

/// Which side of the registry wrote a chunk; a producer write to a settled job is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Writer {
    Producer,
    Pump,
}

/// Whether a kill requested cancellation or found the job already over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillResult {
    Requested,
    AlreadyFinished,
}

impl KillResult {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::AlreadyFinished => "already-finished",
        }
    }
}
