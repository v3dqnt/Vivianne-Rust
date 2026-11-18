//! Repeating a call that failed for a passing reason.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::backoff::{Backoff, random_spread};
use super::failure::Failure;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Calls in all, the first included.
    pub max_attempts: u32,
    pub backoff: Backoff,
    /// Longest the retrying may take in all, waits included, so a failing provider is given up on
    /// (and the next one tried) in about this long rather than minutes.
    pub budget: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 4, backoff: Backoff::default(), budget: Duration::from_secs(90) }
    }
}

impl RetryPolicy {
    /// One call and no retries.
    #[must_use]
    pub fn once() -> Self {
        Self { max_attempts: 1, ..Self::default() }
    }

    /// This policy with at most `max_attempts` calls.
    #[must_use]
    pub fn capped(self, max_attempts: u32) -> Self {
        Self { max_attempts: max_attempts.clamp(1, self.max_attempts.max(1)), ..self }
    }
}

/// Why `retry` stopped without an answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The user stopped the run.
    Cancelled,
    /// The last failure, because it was not worth repeating or the attempts or time ran out.
    Failed(Failure),
}

/// A retry about to happen, for telling the user.
#[derive(Debug)]
pub struct Retrying<'a> {
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay: Duration,
    pub reason: &'a Failure,
}

/// Calls `op(attempt)` until it works, fails for a reason that won't pass, or the policy runs out.
/// Stopping through `cancel` takes effect at once, in the middle of a call or of a wait.
pub async fn retry<T, Fut>(
    policy: &RetryPolicy,
    cancel: &CancellationToken,
    mut on_retry: impl FnMut(&Retrying<'_>),
    mut op: impl FnMut(u32) -> Fut,
) -> Result<T, Stopped>
where
    Fut: Future<Output = Result<T, Failure>>,
{
    let started = Instant::now();
    let mut attempt = 1;
    loop {
        let outcome = tokio::select! {
            () = cancel.cancelled() => return Err(Stopped::Cancelled),
            outcome = op(attempt) => outcome,
        };
        let failure = match outcome {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };
        let delay = policy.backoff.delay(attempt, random_spread());
        let out_of_time = started.elapsed() + delay > policy.budget;
        if !failure.is_transient() || attempt >= policy.max_attempts || out_of_time {
            return Err(Stopped::Failed(failure));
        }
        on_retry(&Retrying { attempt, max_attempts: policy.max_attempts, delay, reason: &failure });
        tokio::select! {
            () = cancel.cancelled() => return Err(Stopped::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const RATE_LIMIT: &str = "429 Too Many Requests: rate limit exceeded";

    fn steady() -> RetryPolicy {
        RetryPolicy { backoff: Backoff { jitter: 0.0, ..Backoff::default() }, ..RetryPolicy::default() }
    }

    #[tokio::test(start_paused = true)]
    async fn a_passing_failure_is_retried_until_it_works() {
        let calls = Cell::new(0);
        let waits = Cell::new(Vec::new());
        let result = retry(
            &steady(),
            &CancellationToken::new(),
            |r| {
                let mut seen = waits.take();
                seen.push(r.delay);
                waits.set(seen);
            },
            |_| {
                calls.set(calls.get() + 1);
                let n = calls.get();
                async move { if n < 3 { Err(Failure::from_message(RATE_LIMIT)) } else { Ok(n) } }
            },
        )
        .await;
        assert_eq!(result, Ok(3));
        assert_eq!(waits.take(), [Duration::from_secs(1), Duration::from_secs(2)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_that_will_not_pass_is_given_up_on_at_once() {
        let calls = Cell::new(0);
        let result: Result<(), Stopped> = retry(&steady(), &CancellationToken::new(), |_| {}, |_| {
            calls.set(calls.get() + 1);
            async { Err(Failure::from_message("400 invalid request")) }
        })
        .await;
        assert!(matches!(result, Err(Stopped::Failed(_))));
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_attempts_run_out() {
        let calls = Cell::new(0);
        let result: Result<(), Stopped> = retry(&steady().capped(3), &CancellationToken::new(), |_| {}, |_| {
            calls.set(calls.get() + 1);
            async { Err(Failure::from_message(RATE_LIMIT)) }
        })
        .await;
        assert!(matches!(result, Err(Stopped::Failed(_))));
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_that_would_pass_the_budget_is_not_started() {
        let policy = RetryPolicy { budget: Duration::from_secs(2), ..steady() };
        let calls = Cell::new(0);
        let result: Result<(), Stopped> = retry(&policy, &CancellationToken::new(), |_| {}, |_| {
            calls.set(calls.get() + 1);
            async { Err(Failure::from_message(RATE_LIMIT)) }
        })
        .await;
        // 1 s wait fits, the following 2 s wait would end past 2 s.
        assert!(matches!(result, Err(Stopped::Failed(_))));
        assert_eq!(calls.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_ends_a_wait_immediately() {
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let stop_soon = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });
        let before = Instant::now();
        let result: Result<(), Stopped> =
            retry(&steady(), &cancel, |_| {}, |_| async { Err(Failure::from_message(RATE_LIMIT)) }).await;
        stop_soon.await.unwrap_or_default();
        assert_eq!(result, Err(Stopped::Cancelled));
        assert!(before.elapsed() < Duration::from_secs(1), "did not sit out the 1 s wait");
    }
}
