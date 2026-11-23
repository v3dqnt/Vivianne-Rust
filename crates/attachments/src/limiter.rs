//! Cancellation and the bound on simultaneous image work. Decoding and encoding are CPU heavy: only a few run at
//! once per store, the rest wait their turn in the order they came.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::error::{Error, Result};

/// Lets a caller stop an operation. Clones share one flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(Cancelled)` once cancelled.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// First come, first served: at most `limit` tasks run at once.
#[derive(Debug)]
pub struct Limiter {
    limit: usize,
    state: Mutex<State>,
    turn: Condvar,
}

#[derive(Debug, Default)]
struct State {
    running: usize,
    next_ticket: u64,
    waiting: VecDeque<u64>,
}

impl Limiter {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            state: Mutex::new(State::default()),
            turn: Condvar::new(),
        }
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Runs `task` once a slot is free. The slot is freed when the task returns, fails or panics.
    pub fn run<T>(&self, task: impl FnOnce() -> T) -> T {
        let _slot = self.acquire();
        task()
    }

    fn acquire(&self) -> Slot<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push_back(ticket);
        while state.running >= self.limit || state.waiting.front() != Some(&ticket) {
            state = self
                .turn
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.waiting.pop_front();
        state.running += 1;
        drop(state);
        // The next in line may fit as well.
        self.turn.notify_all();
        Slot(self)
    }
}

struct Slot<'a>(&'a Limiter);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.running -= 1;
        drop(state);
        self.0.turn.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    #[test]
    fn at_most_the_limit_run_at_once_and_all_finish() {
        let limiter = Arc::new(Limiter::new(2));
        let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (limiter, now, peak) = (limiter.clone(), now.clone(), peak.clone());
                std::thread::spawn(move || {
                    limiter.run(|| {
                        let here = now.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(here, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(15));
                        now.fetch_sub(1, Ordering::SeqCst);
                    });
                })
            })
            .collect();
        for h in handles {
            h.join().expect("task finished");
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(now.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_slot_is_freed_when_a_task_panics() {
        let limiter = Arc::new(Limiter::new(1));
        let l = limiter.clone();
        let _ = std::thread::spawn(move || l.run(|| panic!("task failed"))).join();
        assert_eq!(limiter.run(|| 7), 7);
    }

    #[test]
    fn a_limit_of_zero_still_runs_one() {
        assert_eq!(Limiter::new(0).limit(), 1);
    }

    #[test]
    fn cancel_is_shared_by_clones() {
        let a = Cancel::new();
        let b = a.clone();
        assert!(b.check().is_ok());
        a.cancel();
        assert_eq!(b.check(), Err(Error::Cancelled));
    }
}
