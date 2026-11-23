//! What the queue keeps: jobs, readers, and the books of capacity, caches and indexes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::ordered::Ordered;
use super::types::{Error, Extension, Output, Priority, Source};

pub(super) type JobId = u64;
pub(super) type ReaderId = u64;
pub(super) type Reply = tokio::sync::oneshot::Sender<Result<Output, Error>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum JobState {
    Queued,
    Running,
    Finished,
}

pub(super) struct Job {
    pub(super) extension: Extension,
    pub(super) version: String,
    pub(super) bytes: Option<usize>,
    pub(super) source: Arc<dyn Source>,
    pub(super) cancel: CancellationToken,
    pub(super) readers: HashSet<ReaderId>,
    pub(super) sources: HashSet<String>,
    pub(super) priority: Priority,
    pub(super) state: JobState,
}

pub(super) struct Reader {
    pub(super) job: JobId,
    pub(super) source: String,
    pub(super) priority: Priority,
    pub(super) reply: Option<Reply>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) next_id: u64,
    pub(super) jobs: HashMap<JobId, Job>,
    pub(super) readers: HashMap<ReaderId, Reader>,
    pub(super) queue: Vec<JobId>,
    pub(super) ready: Ordered<Output>,
    /// Source (key, version, extension) to the content it produced.
    pub(super) aliases: Ordered<String>,
    /// Source to the job that will read it.
    pub(super) sources: HashMap<String, JobId>,
    /// Content to the job converting it.
    pub(super) digests: HashMap<String, JobId>,
    pub(super) cached_bytes: usize,
    pub(super) source_bytes: usize,
    pub(super) running: usize,
    pub(super) background: usize,
    pub(super) reader_count: usize,
    pub(super) disposed: bool,
}
