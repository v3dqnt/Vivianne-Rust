//! Routes job events to subscribers, containing each listener.
//!
//! Port of `@deepseek-ai/dsh-jobs-local/events.ts`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::job::lock;
use super::types::{JobEvent, JobEventFilter, JobEventListener};

#[derive(Default)]
pub(super) struct Hub {
    next: AtomicU64,
    subscribers: Mutex<Vec<(u64, JobEventFilter, JobEventListener)>>,
}

impl Hub {
    pub(super) fn emit(&self, event: &JobEvent) {
        let owner = event.owner();
        let matching: Vec<JobEventListener> = lock(&self.subscribers)
            .iter()
            .filter(|(_, filter, _)| match (filter, owner) {
                (JobEventFilter::Owner(wanted), Some(owner)) => wanted == owner,
                _ => true,
            })
            .map(|(_, _, listener)| Arc::clone(listener))
            .collect();
        for listener in matching {
            // An observer cannot break the commit already made.
            if catch_unwind(AssertUnwindSafe(|| listener(event))).is_err() {
                log::warn!("jobs: event listener panicked on {}", event.type_name());
            }
        }
    }
}

impl Hub {
    pub(super) fn subscribe(
        self: &Arc<Self>,
        filter: JobEventFilter,
        listener: JobEventListener,
    ) -> Subscription {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.subscribers).push((id, filter, listener));
        Subscription {
            id,
            hub: Arc::downgrade(self),
        }
    }
}

/// Dropping it unsubscribes.
#[must_use]
pub struct Subscription {
    id: u64,
    hub: std::sync::Weak<Hub>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Subscription")
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            lock(&hub.subscribers).retain(|(id, _, _)| *id != self.id);
        }
    }
}
