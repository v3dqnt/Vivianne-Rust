//! Keeping a run alive when a model call fails: telling passing failures from permanent ones, waiting
//! and trying again, and moving on to a fallback model or key.

mod backoff;
mod chain;
mod failure;
mod retry;

pub use backoff::Backoff;
pub use chain::{Chain, STICKY_FOR};
pub use failure::{Failure, FailureKind};
pub use retry::{Retrying, RetryPolicy, Stopped, retry};
