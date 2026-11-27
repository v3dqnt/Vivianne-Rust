//! The in-process job registry: admission, ownership, output reads, kill, wait, settlement, events, teardown.
//!
//! Port of `@deepseek-ai/dsh-jobs-local/index.ts` and `events.ts`. The original's scope layers, controllers and
//! session records belong to its plugin host and have no counterpart: one toolbox holds one registry.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use super::events::{Hub, Subscription};
use super::job::{State, TrackedJob, Waiting, lock, now_ms};
use super::pump;
use super::ring::OutputRing;
use super::types::{
    JobEvent, JobEventFilter, JobEventListener, JobHandle, JobHooks, JobId, JobOutcome,
    JobOutputRead, JobRead, JobSpec, JobStatus, JobView, KillResult,
};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_active_per_owner: usize,
    pub retain_bytes: usize,
    pub settled_retain_bytes: usize,
    pub pump_every: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active_per_owner: 10,
            retain_bytes: 256 * 1024,
            settled_retain_bytes: 16 * 1024,
            pump_every: Duration::from_millis(150),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("unknown job {0}")]
    Unknown(JobId),
    #[error("job {0} belongs to another session")]
    Foreign(JobId),
    #[error("{0}")]
    Invalid(&'static str),
    #[error(
        "background job limit reached for this owner (limit: {0}); use job_kill to stop an unneeded job, wait for it to finish, then retry"
    )]
    Limit(usize),
    #[error("job {0} is still {1}; kill it and wait for settlement before removing it")]
    StillLive(JobId, JobStatus),
    #[error("cancelling job {0} failed")]
    CancelFailed(JobId),
}

/// The jobs of one process-side owner of work. Dropping it cancels what still runs.
#[derive(Default)]
pub struct Registry {
    limits: Limits,
    jobs: Mutex<Vec<Arc<TrackedJob>>>,
    hub: Arc<Hub>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("jobs", &lock(&self.jobs).len())
            .finish_non_exhaustive()
    }
}

impl Registry {
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            jobs: Mutex::default(),
            hub: Arc::default(),
        }
    }

    pub fn subscribe(&self, filter: JobEventFilter, listener: JobEventListener) -> Subscription {
        self.hub.subscribe(filter, listener)
    }

    /// Admits and starts a job. Needs a tokio runtime.
    pub fn start(&self, spec: JobSpec) -> Result<JobId, JobError> {
        self.admit(spec, false).map(|(id, _)| id)
    }

    /// [`start`](Self::start) with a waiter counted from the first instant, so a job that ends at once still counts
    /// as awaited.
    pub(crate) fn start_waited(&self, spec: JobSpec) -> Result<(JobId, Waiting), JobError> {
        let (id, waiting) = self.admit(spec, true)?;
        waiting
            .map(|waiting| (id.clone(), waiting))
            .ok_or(JobError::Unknown(id))
    }

    fn admit(&self, spec: JobSpec, waited: bool) -> Result<(JobId, Option<Waiting>), JobError> {
        if spec.kind.is_empty() {
            return Err(JobError::Invalid(
                "invalid job kind: expected a non-empty string",
            ));
        }
        if spec.label.is_empty() {
            return Err(JobError::Invalid(
                "invalid job label: expected a non-empty string",
            ));
        }
        if spec.output_limit_bytes == Some(0) {
            return Err(JobError::Invalid(
                "invalid outputLimitBytes: expected a positive integer",
            ));
        }
        let mut jobs = lock(&self.jobs);
        let active = jobs
            .iter()
            .filter(|job| job.owner == spec.owner && !job.is_terminal())
            .count();
        if active >= self.limits.max_active_per_owner {
            return Err(JobError::Limit(self.limits.max_active_per_owner));
        }
        let id = loop {
            let hex = uuid::Uuid::new_v4().simple().to_string();
            let id = format!("{}-{}", spec.kind, &hex[..16]);
            if jobs.iter().all(|job| job.id != id) {
                break id;
            }
        };
        let job = Arc::new(TrackedJob {
            id: id.clone(),
            kind: spec.kind,
            label: spec.label,
            owner: spec.owner,
            output_limit_bytes: spec.output_limit_bytes,
            started_at: now_ms(),
            limits: self.limits,
            hub: Arc::clone(&self.hub),
            settled: watch::channel(false).0,
            state: Mutex::new(State {
                registered: false,
                status: JobStatus::Running,
                ring: OutputRing::new(),
                model_cursor: 0,
                result_delivered: false,
                progress: None,
                detail: None,
                result: None,
                finished_at: None,
                kill_reason: None,
                settle_cause: None,
                cancel: None,
                waiters: 0,
                spill_paths: Vec::new(),
            }),
        });
        let handle = JobHandle {
            id: id.clone(),
            job: Arc::downgrade(&job),
        };
        let JobHooks { done, cancel } = (spec.run)(handle);
        let view = {
            let mut state = lock(&job.state);
            state.cancel = Some(cancel);
            state.registered = true;
            job.view_of(&state)
        };
        let waiting = waited.then(|| Waiting::new(Arc::clone(&job)));
        jobs.push(Arc::clone(&job));
        drop(jobs);
        // The first event of a job is always `registered`.
        self.hub.emit(&JobEvent::Registered(view));
        drive(job, done, spec.output);
        Ok((id, waiting))
    }

    fn expect(&self, id: &str, caller: Option<&str>) -> Result<Arc<TrackedJob>, JobError> {
        let job = lock(&self.jobs)
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or_else(|| JobError::Unknown(id.to_owned()))?;
        match &job.owner {
            Some(owner) if Some(owner.as_str()) != caller => Err(JobError::Foreign(job.id.clone())),
            _ => Ok(job),
        }
    }

    /// Unowned jobs and the caller's own, oldest first.
    #[must_use]
    pub fn list(&self, caller: Option<&str>) -> Vec<JobView> {
        lock(&self.jobs)
            .iter()
            .filter(|job| job.owner.is_none() || job.owner.as_deref() == caller)
            .map(|job| job.view())
            .collect()
    }

    pub fn get(&self, id: &str, caller: Option<&str>) -> Result<JobView, JobError> {
        Ok(self.expect(id, caller)?.view())
    }

    /// Everything since the last such read, which it moves on from; a result is handed out once.
    pub fn read(&self, id: &str, caller: Option<&str>) -> Result<JobRead, JobError> {
        let job = self.expect(id, caller)?;
        let mut state = lock(&job.state);
        let read = state.ring.read_from(state.model_cursor);
        state.model_cursor = state.ring.total;
        let terminal = state.status.is_terminal();
        let result = (terminal && !state.result_delivered)
            .then(|| state.result.clone())
            .flatten();
        state.result_delivered |= result.is_some();
        if terminal {
            state.ring.trim(job.limits.settled_retain_bytes);
        }
        Ok(JobRead {
            chunks: read.chunks,
            lossy: read.lossy,
            result,
            job: job.view_of(&state),
        })
    }

    /// What is retained from `from` on, moving nothing.
    pub fn read_at(
        &self,
        id: &str,
        from: u64,
        caller: Option<&str>,
    ) -> Result<JobOutputRead, JobError> {
        let job = self.expect(id, caller)?;
        let state = lock(&job.state);
        Ok(state.ring.read_from(from))
    }

    pub fn kill(
        &self,
        id: &str,
        caller: Option<&str>,
        reason: Option<&str>,
    ) -> Result<KillResult, JobError> {
        self.expect(id, caller)?.kill(reason)
    }

    /// The job once it settles or `limit` passes. Dropping the future gives up the wait only.
    pub async fn wait(
        &self,
        id: &str,
        limit: Duration,
        caller: Option<&str>,
    ) -> Result<JobView, JobError> {
        if limit.is_zero() {
            return Err(JobError::Invalid(
                "invalid wait timeout: expected a positive number of milliseconds",
            ));
        }
        let job = self.expect(id, caller)?;
        if job.is_terminal() {
            return Ok(job.view());
        }
        Ok(Waiting::new(job).wait(limit).await)
    }

    /// Forgets a finished job.
    pub fn remove(&self, id: &str, caller: Option<&str>) -> Result<(), JobError> {
        let job = self.expect(id, caller)?;
        let view = job.view();
        if !view.status.is_terminal() {
            return Err(JobError::StillLive(view.id, view.status));
        }
        self.drop_jobs(&[job]);
        Ok(())
    }

    fn drop_jobs(&self, gone: &[Arc<TrackedJob>]) {
        lock(&self.jobs).retain(|job| gone.iter().all(|other| !Arc::ptr_eq(job, other)));
        for job in gone {
            self.hub.emit(&JobEvent::Removed(job.view()));
        }
    }

    async fn tear_down(&self, jobs: Vec<Arc<TrackedJob>>, reason: &str) {
        for job in &jobs {
            job.cancel_for_teardown(reason);
        }
        for job in &jobs {
            let mut settled = job.settled.subscribe();
            let _ = settled.wait_for(|done| *done).await;
        }
        self.drop_jobs(&jobs);
    }

    /// Cancels, awaits and forgets one owner's jobs.
    pub async fn dispose_owner(&self, owner: &str) {
        let owned = lock(&self.jobs)
            .iter()
            .filter(|job| job.owner.as_deref() == Some(owner))
            .cloned()
            .collect();
        self.tear_down(owned, "owner disposed").await;
    }

    /// Cancels, awaits and forgets every job.
    pub async fn shutdown(&self) {
        let all = lock(&self.jobs).clone();
        self.tear_down(all, "jobs service disposed").await;
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        // Nothing can await here: ask everything to stop and let the producers finish on their own.
        for job in lock(&self.jobs).iter() {
            job.cancel_for_teardown("jobs service disposed");
        }
    }
}

/// Follows the producer: pumps its sources while it runs, then settles.
fn drive(
    job: Arc<TrackedJob>,
    done: futures::future::BoxFuture<'static, JobOutcome>,
    sources: Vec<Box<dyn super::types::JobOutputSource>>,
) {
    let (finished, watching) = watch::channel(false);
    let pump = (!sources.is_empty()).then(|| {
        tokio::spawn(pump::run(
            Arc::clone(&job),
            sources,
            job.limits.pump_every,
            watching,
        ))
    });
    tokio::spawn(async move {
        // A producer that panics is a failed job, not a stuck one.
        let outcome = tokio::spawn(done)
            .await
            .unwrap_or_else(|error| JobOutcome::failed(error.to_string()));
        finished.send_replace(true);
        if let Some(pump) = pump {
            let _ = pump.await;
        }
        job.settle(outcome, None);
    });
}
