//! Bounded source admission, shared content conversion, and caller-owned PDF delivery. One converter generation owns every
//! queued source, conversion, reader and retained PDF. Port of the original `queue.ts`.
//!
//! Metadata is admitted before any source byte is read. Readers of the same source, and of different sources with the same
//! bytes, share one conversion; the last reader to leave cancels it. Finished PDFs are kept by content, with a bounded
//! index from source version to content so a known source is not read again.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::state::{Job, JobId, JobState, Reader, ReaderId, State};
use super::types::{Code, Config, Converter, Error, Output, Priority, Request};

struct Inner {
    config: Config,
    generation: String,
    convert: Arc<dyn Converter>,
    state: Mutex<State>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// What admitting a request came to.
enum Admitted {
    /// A finished PDF that was already kept.
    Ready(Output),
    Waiting(oneshot::Receiver<Result<Output, Error>>, ReaderId),
}

/// The queue of one converter generation.
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("generation", &self.inner.generation)
            .finish_non_exhaustive()
    }
}

impl Queue {
    /// `generation` is the converter's lifetime: it keeps a PDF from being reused after the engine or its settings change.
    #[must_use]
    pub fn new(config: Config, generation: String, convert: Arc<dyn Converter>) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                generation,
                convert,
                state: Mutex::new(State::default()),
                tasks: Mutex::new(Vec::new()),
            }),
        }
    }

    #[must_use]
    pub fn generation(&self) -> &str {
        &self.inner.generation
    }

    /// Admits the request's metadata before any source byte is loaded, and shares conversion across readers. Independent
    /// PDF bytes come back; a `busy` or cancelled reader gives up its place without freeing engine capacity early.
    pub async fn read(
        &self,
        request: Request,
        cancel: &CancellationToken,
    ) -> Result<Output, Error> {
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let admitted = self.inner.admit(&request)?;
        let (rx, reader) = match admitted {
            Admitted::Ready(output) => return Ok(output),
            Admitted::Waiting(rx, reader) => (rx, reader),
        };
        Inner::drain(&self.inner);
        tokio::select! {
            answer = rx => answer.unwrap_or_else(|_| Err(Error::unavailable())),
            () = cancel.cancelled() => {
                self.inner.abandon(reader);
                Inner::drain(&self.inner);
                Err(Error::cancelled())
            }
        }
    }

    /// Cancels every reader and waits until actual reads, conversions and scratch cleanup have finished.
    pub async fn dispose(&self) {
        {
            let mut state = self.inner.lock();
            state.disposed = true;
            let ids: Vec<JobId> = state.jobs.keys().copied().collect();
            for id in ids {
                Inner::fail(&mut state, id, &Error::unavailable());
            }
            state.ready.clear();
            state.aliases.clear();
            state.cached_bytes = 0;
        }
        let tasks = std::mem::take(
            &mut *self
                .inner
                .tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn reservation(&self, job: &Job) -> usize {
        job.bytes.unwrap_or(self.config.max_input_bytes).max(1)
    }

    fn admit(&self, request: &Request) -> Result<Admitted, Error> {
        let config = &self.config;
        if request.bytes.is_some_and(|b| b > config.max_input_bytes) {
            return Err(Error::new(
                Code::InputTooLarge,
                "The Office source exceeds maxInputBytes.",
            ));
        }
        let mut state = self.lock();
        if state.disposed {
            return Err(Error::unavailable());
        }
        let source = format!(
            "{}\0{}\0{}",
            request.source_key,
            request.version,
            request.extension.as_str()
        );
        if let Some(key) = state.aliases.get(&source).cloned()
            && let Some(cached) = state.ready.get(&key).cloned()
        {
            state.ready.touch(&key);
            state.aliases.touch(&source);
            return Ok(Admitted::Ready(cached));
        }
        let background = request.priority == Priority::Background;
        // The last reader slot is kept for foreground interest.
        let reader_limit = if background {
            config.max_readers.saturating_sub(1)
        } else {
            config.max_readers
        };
        if state.reader_count >= reader_limit
            || (background && config.max_background_conversions == 0)
        {
            return Err(Error::busy());
        }
        let job_id = if let Some(id) = state.sources.get(&source).copied() {
            id
        } else {
            {
                if state.queue.len() >= config.max_queued_jobs {
                    // Foreground work may displace queued speculation; nothing else makes room.
                    let obsolete = (!background)
                        .then(|| {
                            state
                                .queue
                                .iter()
                                .copied()
                                .find(|id| state.jobs[id].priority == Priority::Background)
                        })
                        .flatten();
                    let Some(obsolete) = obsolete else {
                        return Err(Error::busy());
                    };
                    Self::fail(&mut state, obsolete, &Error::busy());
                }
                state.next_id += 1;
                let id = state.next_id;
                state.jobs.insert(
                    id,
                    Job {
                        extension: request.extension,
                        version: request.version.clone(),
                        bytes: request.bytes,
                        source: request.source.clone(),
                        cancel: CancellationToken::new(),
                        readers: HashSet::new(),
                        sources: HashSet::from([source.clone()]),
                        priority: request.priority,
                        state: JobState::Queued,
                    },
                );
                state.sources.insert(source.clone(), id);
                state.queue.push(id);
                id
            }
        };
        if let Some(job) = state.jobs.get_mut(&job_id)
            && !background
        {
            job.priority = Priority::Foreground;
        }
        state.reader_count += 1;
        state.next_id += 1;
        let reader_id = state.next_id;
        let (tx, rx) = oneshot::channel();
        state.readers.insert(
            reader_id,
            Reader {
                job: job_id,
                source,
                priority: request.priority,
                reply: Some(tx),
            },
        );
        if let Some(job) = state.jobs.get_mut(&job_id) {
            job.readers.insert(reader_id);
        }
        Ok(Admitted::Waiting(rx, reader_id))
    }

    /// A reader stopped waiting: it leaves, and the job is cancelled when nobody else wants it.
    fn abandon(&self, reader: ReaderId) {
        let mut state = self.lock();
        let Some(job) = Self::release(&mut state, reader) else {
            return;
        };
        if state.jobs.get(&job).is_some_and(|j| j.readers.is_empty()) {
            Self::cancel(&mut state, job);
        }
    }

    /// Removes a reader from its job and the books. The reader's reply, if still unsent, is returned to the caller.
    fn release(state: &mut State, id: ReaderId) -> Option<JobId> {
        let reader = state.readers.remove(&id)?;
        state.reader_count = state.reader_count.saturating_sub(1);
        let job_id = reader.job;
        let (shares_source, foreground_left) = match state.jobs.get_mut(&job_id) {
            Some(job) => {
                job.readers.remove(&id);
                let foreground_left = job.readers.iter().any(|r| {
                    state
                        .readers
                        .get(r)
                        .is_some_and(|o| o.priority == Priority::Foreground)
                });
                let shares = job.readers.iter().any(|r| {
                    state
                        .readers
                        .get(r)
                        .is_some_and(|o| o.source == reader.source)
                });
                if job.state == JobState::Queued {
                    job.priority = if foreground_left {
                        Priority::Foreground
                    } else {
                        Priority::Background
                    };
                }
                if !shares {
                    job.sources.remove(&reader.source);
                }
                (shares, foreground_left)
            }
            None => (true, false),
        };
        let _ = foreground_left;
        if !shares_source && state.sources.get(&reader.source) == Some(&job_id) {
            state.sources.remove(&reader.source);
        }
        Some(job_id)
    }

    fn cancel(state: &mut State, id: JobId) {
        let Some(job) = state.jobs.get_mut(&id) else {
            return;
        };
        job.cancel.cancel();
        if job.state == JobState::Queued {
            job.state = JobState::Finished;
            state.queue.retain(|queued| *queued != id);
            state.jobs.remove(&id);
        }
    }

    /// Rejects every reader of a job with `error`, then cancels it.
    fn fail(state: &mut State, id: JobId, error: &Error) {
        let readers: Vec<ReaderId> = state
            .jobs
            .get(&id)
            .map(|j| j.readers.iter().copied().collect())
            .unwrap_or_default();
        for reader in readers {
            let reply = state.readers.get_mut(&reader).and_then(|r| r.reply.take());
            Self::release(state, reader);
            if let Some(reply) = reply {
                let _ = reply.send(Err(error.clone()));
            }
        }
        Self::cancel(state, id);
    }

    /// Starts every queued job that may start now.
    fn drain(this: &Arc<Self>) {
        let mut started = Vec::new();
        {
            let mut state = this.lock();
            let config = &this.config;
            while !state.disposed && state.running < config.max_concurrent_conversions {
                let foreground_waiting = state
                    .queue
                    .iter()
                    .any(|id| state.jobs[id].priority == Priority::Foreground);
                let background_room = state.background
                    < config
                        .max_background_conversions
                        .min(config.max_concurrent_conversions.saturating_sub(1).max(1));
                let eligible = |id: &JobId| {
                    let job = &state.jobs[id];
                    state.source_bytes + this.reservation(job) <= config.max_source_bytes
                        && (job.priority == Priority::Foreground || background_room)
                };
                let pick = state.queue.iter().copied().find(|id| {
                    (!foreground_waiting || state.jobs[id].priority == Priority::Foreground)
                        && eligible(id)
                });
                let Some(id) = pick else { break };
                state.queue.retain(|queued| *queued != id);
                let (background, reserved) = {
                    let job = &state.jobs[&id];
                    (job.priority == Priority::Background, this.reservation(job))
                };
                if let Some(job) = state.jobs.get_mut(&id) {
                    job.state = JobState::Running;
                }
                state.running += 1;
                if background {
                    state.background += 1;
                }
                state.source_bytes += reserved;
                started.push((id, reserved, background));
            }
        }
        for (id, reserved, background) in started {
            let inner = this.clone();
            let handle = tokio::spawn(async move {
                if let Err(error) = Self::execute(&inner, id, reserved).await {
                    Self::fail(&mut inner.lock(), id, &error);
                }
                {
                    let mut state = inner.lock();
                    state.running = state.running.saturating_sub(1);
                    if background {
                        state.background = state.background.saturating_sub(1);
                    }
                    state.source_bytes = state.source_bytes.saturating_sub(reserved);
                    if let Some(job) = state.jobs.get_mut(&id) {
                        job.state = JobState::Finished;
                    }
                    state.jobs.remove(&id);
                }
                Self::drain(&inner);
            });
            let mut tasks = this.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            tasks.retain(|task| !task.is_finished());
            tasks.push(handle);
        }
    }

    async fn execute(this: &Arc<Self>, id: JobId, reserved: usize) -> Result<(), Error> {
        let (source, extension, version, cancel) = {
            let state = this.lock();
            let Some(job) = state.jobs.get(&id) else {
                return Err(Error::cancelled());
            };
            (
                job.source.clone(),
                job.extension,
                job.version.clone(),
                job.cancel.clone(),
            )
        };
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        // The source and the engine are awaited, not raced against cancellation: each stops itself when the token fires and
        // cleans up (scratch folders, child processes) before it answers, and disposal waits for exactly that.
        let read = source.read(&cancel, reserved).await?;
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let (bytes, read_version) = read;
        if read_version != version {
            return Err(Error::new(
                Code::SourceChanged,
                "The source changed while waiting for conversion.",
            ));
        }
        if bytes.len() > reserved {
            return Err(Error::new(
                Code::InputTooLarge,
                "The source exceeds its reserved read capacity.",
            ));
        }
        let digest = Sha256::new()
            .chain_update(extension.as_str())
            .chain_update([0u8])
            .chain_update(&bytes)
            .finalize();
        let hex = digest.iter().fold(String::new(), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        });
        let key = format!("{}:{hex}", this.generation);
        {
            let mut state = this.lock();
            if let Some(cached) = state.ready.get(&key).cloned() {
                state.ready.touch(&key);
                Self::finish(&mut state, id, &cached, &this.config);
                return Ok(());
            }
            if let Some(existing) = state.digests.get(&key).copied()
                && state
                    .jobs
                    .get(&existing)
                    .is_some_and(|j| !j.cancel.is_cancelled())
            {
                Self::merge_into(&mut state, id, existing);
                return Ok(());
            }
            state.digests.insert(key.clone(), id);
        }
        let outcome = async {
            let converted = this.convert.convert(&bytes, extension, &cancel).await?;
            if cancel.is_cancelled() {
                return Err(Error::cancelled());
            }
            let output = Output {
                pdf: converted.pdf,
                missing_fonts: converted.missing_fonts,
                cache_key: key.clone(),
                generation: this.generation.clone(),
            };
            let mut state = this.lock();
            Self::retain(&mut state, &output, &this.config);
            Self::finish(&mut state, id, &output, &this.config);
            Ok(())
        }
        .await;
        let mut state = this.lock();
        if state.digests.get(&key) == Some(&id) {
            state.digests.remove(&key);
        }
        outcome
    }

    /// The conversion this job would have made is already running: its readers and source indexes go there.
    fn merge_into(state: &mut State, id: JobId, existing: JobId) {
        let Some((priority, readers, sources)) = state.jobs.get_mut(&id).map(|job| {
            (
                job.priority,
                std::mem::take(&mut job.readers),
                std::mem::take(&mut job.sources),
            )
        }) else {
            return;
        };
        if let Some(target) = state.jobs.get_mut(&existing) {
            if priority == Priority::Foreground {
                target.priority = Priority::Foreground;
            }
            target.readers.extend(readers.iter().copied());
            target.sources.extend(sources.iter().cloned());
        }
        for reader in readers {
            if let Some(r) = state.readers.get_mut(&reader) {
                r.job = existing;
            }
        }
        for source in sources {
            state.sources.insert(source, existing);
        }
    }

    fn finish(state: &mut State, id: JobId, result: &Output, config: &Config) {
        let sources: Vec<String> = state
            .jobs
            .get(&id)
            .map(|j| j.sources.iter().cloned().collect())
            .unwrap_or_default();
        for source in &sources {
            state.sources.remove(source);
        }
        if state.ready.contains(&result.cache_key) {
            for source in &sources {
                state
                    .aliases
                    .insert(source.clone(), result.cache_key.clone());
                while state.aliases.len() > config.max_source_entries {
                    state.aliases.pop_oldest();
                }
            }
        }
        let readers: Vec<ReaderId> = state
            .jobs
            .get(&id)
            .map(|j| j.readers.iter().copied().collect())
            .unwrap_or_default();
        for reader in readers {
            let reply = state.readers.get_mut(&reader).and_then(|r| r.reply.take());
            Self::release(state, reader);
            if let Some(reply) = reply {
                let _ = reply.send(Ok(result.clone()));
            }
        }
    }

    /// Keeps a PDF, least-recently-used out at either limit together with the aliases that point at it.
    fn retain(state: &mut State, result: &Output, config: &Config) {
        if result.pdf.len() > config.max_cached_bytes {
            return;
        }
        while state.ready.len() >= config.max_cached_entries
            || state.cached_bytes + result.pdf.len() > config.max_cached_bytes
        {
            let Some((key, oldest)) = state.ready.pop_oldest() else {
                break;
            };
            state.cached_bytes = state.cached_bytes.saturating_sub(oldest.pdf.len());
            let stale: Vec<String> = state
                .aliases
                .map
                .iter()
                .filter(|(_, digest)| **digest == key)
                .map(|(source, _)| source.clone())
                .collect();
            for source in stale {
                state.aliases.remove(&source);
            }
        }
        state.cached_bytes += result.pdf.len();
        state.ready.insert(result.cache_key.clone(), result.clone());
    }
}
