//! One job's record: its ring, status, settlement, kill and waiters.
//!
//! Port of the per-job half of `@deepseek-ai/dsh-jobs-local/index.ts`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::watch;

use super::events::Hub;
use super::registry::{JobError, Limits};
use super::ring::OutputRing;
use super::types::{
    JobAppendOptions, JobEvent, JobId, JobOutcome, JobOutputView, JobSettleCause, JobStatus,
    JobView, KillResult, Writer,
};

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type Cancel = Arc<dyn Fn(Option<&str>) + Send + Sync>;

pub(super) struct State {
    /// Set at admission; events are held back before it.
    pub(super) registered: bool,
    pub(super) status: JobStatus,
    pub(super) ring: OutputRing,
    pub(super) model_cursor: u64,
    pub(super) result_delivered: bool,
    pub(super) progress: Option<String>,
    pub(super) detail: Option<String>,
    pub(super) result: Option<String>,
    pub(super) finished_at: Option<u64>,
    pub(super) kill_reason: Option<String>,
    pub(super) settle_cause: Option<JobSettleCause>,
    pub(super) cancel: Option<Cancel>,
    pub(super) waiters: usize,
    pub(super) spill_paths: Vec<Option<String>>,
}

/// One job's record. Producers and the pump reach it through [`JobHandle`].
pub(crate) struct TrackedJob {
    pub(super) id: JobId,
    pub(super) kind: String,
    pub(super) label: String,
    pub(super) owner: Option<String>,
    pub(super) output_limit_bytes: Option<usize>,
    pub(super) started_at: u64,
    pub(super) limits: Limits,
    pub(super) hub: Arc<Hub>,
    pub(super) state: Mutex<State>,
    pub(super) settled: watch::Sender<bool>,
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl TrackedJob {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn view_of(&self, state: &State) -> JobView {
        let mut spill_paths: Vec<String> = Vec::new();
        for path in state.spill_paths.iter().flatten() {
            if !spill_paths.contains(path) {
                spill_paths.push(path.clone());
            }
        }
        JobView {
            id: self.id.clone(),
            kind: self.kind.clone(),
            label: self.label.clone(),
            owner: self.owner.clone(),
            output_limit_bytes: self.output_limit_bytes,
            status: state.status,
            progress: state.progress.clone(),
            detail: state.detail.clone(),
            started_at: self.started_at,
            finished_at: state.finished_at,
            output: JobOutputView {
                total: state.ring.total,
                earliest: state.ring.earliest,
                spill_paths: (!spill_paths.is_empty()).then_some(spill_paths),
            },
        }
    }

    pub(super) fn view(&self) -> JobView {
        self.view_of(&lock(&self.state))
    }

    fn output_event(&self, total: u64) -> JobEvent {
        JobEvent::Output {
            id: self.id.clone(),
            owner: self.owner.clone(),
            total,
        }
    }

    pub(super) fn append(&self, text: &str, options: &JobAppendOptions, writer: Writer) {
        let event = {
            let mut state = lock(&self.state);
            if state.status.is_terminal() {
                if writer == Writer::Producer {
                    log::warn!("jobs: append to settled job {} dropped", self.id);
                }
                return;
            }
            if !state.ring.append(text, options, self.limits.retain_bytes) {
                return;
            }
            state
                .registered
                .then(|| self.output_event(state.ring.total))
        };
        if let Some(event) = event {
            self.hub.emit(&event);
        }
    }

    pub(super) fn update_progress(&self, line: String) {
        let view = {
            let mut state = lock(&self.state);
            if state.status.is_terminal() {
                log::warn!("jobs: progress update on settled job {} dropped", self.id);
                return;
            }
            state.progress = Some(line);
            state.registered.then(|| self.view_of(&state))
        };
        if let Some(view) = view {
            self.hub.emit(&JobEvent::Progress(view));
        }
    }

    pub(super) fn spill(&self, index: usize, path: Option<String>) {
        let mut state = lock(&self.state);
        if state.spill_paths.len() <= index {
            state.spill_paths.resize(index + 1, None);
        }
        state.spill_paths[index] = path;
    }

    /// First settlement wins.
    pub(super) fn settle(&self, outcome: JobOutcome, cause: Option<JobSettleCause>) {
        let (view, cause, awaited, total) = {
            let mut state = lock(&self.state);
            if state.status.is_terminal() {
                return;
            }
            let cause = cause
                .or(state.settle_cause)
                .unwrap_or(JobSettleCause::Producer);
            // A producer cannot report itself live.
            state.status = if outcome.status.is_terminal() {
                outcome.status
            } else {
                JobStatus::Failed
            };
            // Producer facts first, then why it was killed. A job that outran its kill keeps the producer's alone.
            state.detail = match (&outcome.detail, &state.kill_reason, state.status) {
                (Some(detail), Some(reason), JobStatus::Killed) => {
                    Some(format!("{detail}; {reason}"))
                }
                (None, Some(reason), JobStatus::Killed) => Some(reason.clone()),
                (detail, _, _) => detail.clone(),
            };
            state.progress = None;
            state.result = outcome.result;
            state.finished_at = Some(now_ms());
            state.cancel = None;
            // Never below what the model has not read yet.
            let unread =
                usize::try_from(state.ring.total - state.model_cursor).unwrap_or(usize::MAX);
            state
                .ring
                .trim(self.limits.settled_retain_bytes.max(unread));
            (
                self.view_of(&state),
                cause,
                state.waiters > 0,
                state.ring.total,
            )
        };
        self.settled.send_replace(true);
        self.hub.emit(&JobEvent::Settled {
            job: view,
            cause,
            awaited,
        });
        self.hub.emit(&self.output_event(total));
    }

    pub(super) fn is_terminal(&self) -> bool {
        lock(&self.state).status.is_terminal()
    }

    pub(super) fn kill(&self, reason: Option<&str>) -> Result<KillResult, JobError> {
        let cancel = {
            let state = lock(&self.state);
            if state.status.is_terminal() {
                return Ok(KillResult::AlreadyFinished);
            }
            state.cancel.clone()
        };
        // Cancel first, so a cancel that fails leaves the job as it was.
        if let Some(cancel) = cancel
            && catch_unwind(AssertUnwindSafe(|| cancel(reason))).is_err()
        {
            return Err(JobError::CancelFailed(self.id.clone()));
        }
        let view = {
            let mut state = lock(&self.state);
            if let Some(reason) = reason {
                state.kill_reason = Some(reason.to_owned());
            }
            state.settle_cause = Some(JobSettleCause::Kill);
            if state.status.is_terminal() {
                return Ok(KillResult::Requested);
            }
            state.status = JobStatus::Stopping;
            self.view_of(&state)
        };
        self.hub.emit(&JobEvent::Stopping(view));
        Ok(KillResult::Requested)
    }

    pub(super) fn cancel_for_teardown(&self, reason: &str) {
        let cancel = {
            let mut state = lock(&self.state);
            if state.status.is_terminal() {
                return;
            }
            // Announced as teardown, so nobody reports completion to a reader that is going away.
            state.settle_cause = Some(JobSettleCause::Teardown);
            state.cancel.clone()
        };
        let Some(cancel) = cancel else { return };
        if catch_unwind(AssertUnwindSafe(|| cancel(Some(reason)))).is_err() {
            let detail = "cancel threw during teardown; work may be orphaned";
            log::warn!("jobs: {}: {detail}", self.id);
            self.settle(JobOutcome::failed(detail), Some(JobSettleCause::Teardown));
            return;
        }
        let view = {
            let mut state = lock(&self.state);
            if state.status.is_terminal() {
                return;
            }
            state.status = JobStatus::Stopping;
            self.view_of(&state)
        };
        self.hub.emit(&JobEvent::Stopping(view));
    }

    /// Waits for settlement. The caller has already been counted as a waiter.
    async fn settlement(&self, limit: Duration) {
        let mut settled = self.settled.subscribe();
        let _ = tokio::time::timeout(limit, settled.wait_for(|done| *done)).await;
    }
}

/// One counted waiter; a settlement while one exists was awaited.
pub(crate) struct Waiting(Arc<TrackedJob>);

impl Waiting {
    pub(super) fn new(job: Arc<TrackedJob>) -> Self {
        lock(&job.state).waiters += 1;
        Self(job)
    }

    /// The job's state once it settles or `limit` passes, whichever is first.
    pub(crate) async fn wait(self, limit: Duration) -> JobView {
        self.0.settlement(limit).await;
        self.0.view()
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.waiters = state.waiters.saturating_sub(1);
    }
}
