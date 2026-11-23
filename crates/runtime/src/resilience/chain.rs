//! The order in which to try the places a request can go: the chosen model first, then fallbacks
//! (other models, or the same model on another key).

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// How long a run keeps using the fallback that rescued it before trying its own model again.
/// Going back to a model that is limited for the next hour would pay a full retry cycle every
/// turn just to learn that again.
pub const STICKY_FOR: Duration = Duration::from_mins(30);

/// The targets, in preference order, and which one last rescued the run.
#[derive(Debug)]
pub struct Chain {
    len: usize,
    rescuer: Mutex<Option<(usize, Instant)>>,
}

impl Chain {
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            len,
            rescuer: Mutex::new(None),
        }
    }

    /// Indexes of the targets in the order to try them now: a recent rescuer first, then the rest
    /// in their own order.
    #[must_use]
    pub fn order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.len).collect();
        let rescuer = *self
            .rescuer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((index, since)) = rescuer
            && since.elapsed() < STICKY_FOR
            && index < self.len
        {
            order.retain(|i| *i != index);
            order.insert(0, index);
        }
        order
    }

    /// Remembers which target answered. Only a fallback sticks: the first target answering is the
    /// normal case and clears any earlier rescuer.
    pub fn answered(&self, index: usize) {
        let mut rescuer = self
            .rescuer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *rescuer = (index != 0).then(|| (index, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_order_is_the_configured_one_until_a_fallback_rescues_the_run() {
        let chain = Chain::new(3);
        assert_eq!(chain.order(), [0, 1, 2]);
        chain.answered(2);
        assert_eq!(chain.order(), [2, 0, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rescuer_is_forgotten_after_its_time_or_when_the_first_target_works_again() {
        let chain = Chain::new(2);
        chain.answered(1);
        tokio::time::advance(STICKY_FOR + Duration::from_secs(1)).await;
        assert_eq!(chain.order(), [0, 1]);
        chain.answered(1);
        chain.answered(0);
        assert_eq!(chain.order(), [0, 1]);
    }
}
