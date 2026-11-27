//! Background jobs: work that outlives the tool call that started it. [`Registry`] owns the jobs, [`Jobs`] is one
//! toolbox's face of it (what the model did, completion notices, a wait the person interrupted).
//!
//! Port of `@deepseek-ai/dsh-jobs`, `dsh-jobs-local` and `dsh-tool-jobs`.

mod events;
mod job;
mod pump;
mod registry;
mod render;
mod ring;
mod tools;
mod types;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub use events::Subscription;
pub use registry::{JobError, Limits, Registry};
pub use tools::{JobKill, JobList, JobOutput};
pub use types::*;

pub(crate) use render::{delta, status_line};

use job::lock;

/// A job whose whole output is at most this is quoted in its completion notice.
const INLINE_BYTES: u64 = 2048;

/// What the model is told about background work.
pub const GUIDANCE: &str = "Track every background job id you start. You are notified in-session when a job \
finishes — do not busy-poll or sleep on one; keep working on independent steps and do not duplicate a running \
job's work. Before giving a final answer, collect every still-relevant job with job_output (set wait: true only \
when you are genuinely blocked on it), and job_kill jobs that stopped mattering.";

#[derive(Debug, Default)]
struct Shared {
    registry: Registry,
    /// Jobs the model stopped itself: their end is no news to it.
    killed: Mutex<HashSet<JobId>>,
    steered: tokio::sync::Notify,
    /// Where the whole output of a job goes once it outgrows memory; nowhere until a surface says.
    spill: std::sync::OnceLock<std::path::PathBuf>,
}

/// Whole-output files kept; older ones go when a new job starts.
const KEPT_SPILLS: usize = 20;

/// One toolbox's background jobs. Clones share them; they are cancelled when the last clone goes.
#[derive(Debug, Clone, Default)]
pub struct Jobs(Arc<Shared>);

impl Jobs {
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self(Arc::new(Shared {
            registry: Registry::new(limits),
            ..Shared::default()
        }))
    }

    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.0.registry
    }

    /// Keeps jobs' whole output in files under `dir`. The first call decides.
    pub fn keep_output_in(&self, dir: std::path::PathBuf) {
        let _ = self.0.spill.set(dir);
    }

    /// [`keep_output_in`](Self::keep_output_in) the folder beside the stored tool results.
    pub fn keep_output(&self) {
        self.keep_output_in(crate::results::dir().join("jobs"));
    }

    /// The spill folder, made and pruned, ready for one more job.
    pub(crate) fn spill_dir(&self) -> Option<std::path::PathBuf> {
        let dir = self.0.spill.get()?;
        std::fs::create_dir_all(dir).ok()?;
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        files.sort();
        let spare = files.len().saturating_sub(KEPT_SPILLS);
        for (_, path) in files.into_iter().take(spare) {
            // One still being written (Windows) or already gone is left to the next round.
            let _ = std::fs::remove_file(path);
        }
        Some(dir.clone())
    }

    /// Notes that the model asked for `id` to stop, so no completion notice follows.
    pub(crate) fn mark_killed(&self, id: &str) {
        lock(&self.0.killed).insert(id.to_owned());
    }

    /// The person said something: a wait the model is in gives way.
    pub fn steered(&self) {
        self.0.steered.notify_waiters();
    }

    pub(crate) async fn until_steered(&self) {
        self.0.steered.notified().await;
    }

    /// Calls `say` with the message to put in the conversation when a job ends and the model does not already know.
    pub fn on_notice(&self, say: impl Fn(String) + Send + Sync + 'static) -> Subscription {
        let shared = Arc::downgrade(&self.0);
        self.0.registry.subscribe(
            JobEventFilter::All,
            Arc::new(move |event| {
                let JobEvent::Settled {
                    job,
                    cause,
                    awaited,
                } = event
                else {
                    return;
                };
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                if *cause == JobSettleCause::Teardown
                    || *awaited
                    || lock(&shared.killed).contains(&job.id)
                {
                    return;
                }
                say(render::notice(&shared.registry, job, INLINE_BYTES));
            }),
        )
    }
}
