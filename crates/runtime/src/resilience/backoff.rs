//! How long to wait before the next attempt.

use std::time::Duration;

/// Exponential backoff with jitter. The first retry waits `base`, the next `2 × base`, and so on up
/// to `cap`; jitter spreads out many runs that were all limited in the same moment.
///
/// (The old loop's comment promised 1 s, 2 s, 4 s, 8 s but its arithmetic gave 2 s, 4 s, 8 s, 16 s.)
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub base: Duration,
    pub cap: Duration,
    /// Fraction of the delay to vary by, either way (0.2 is ±20%).
    pub jitter: f64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { base: Duration::from_secs(1), cap: Duration::from_secs(16), jitter: 0.2 }
    }
}

impl Backoff {
    /// The wait after failed attempt number `attempt` (counting from 1). `spread` is a number in
    /// `-1.0..=1.0` that places the delay inside its jitter band; callers pass [`random_spread`].
    #[must_use]
    pub fn delay(&self, attempt: u32, spread: f64) -> Duration {
        let doublings = attempt.saturating_sub(1).min(16);
        let grown = self.base.saturating_mul(1 << doublings).min(self.cap);
        let factor = 1.0 + self.jitter * spread.clamp(-1.0, 1.0);
        grown.mul_f64(factor.max(0.0))
    }
}

/// A fresh number in `-1.0..=1.0`.
#[must_use]
pub fn random_spread() -> f64 {
    rand::random_range(-1.0..=1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_wait_doubles_from_the_base_and_stops_at_the_cap() {
        let backoff = Backoff::default();
        let waits: Vec<_> = (1..=7).map(|attempt| backoff.delay(attempt, 0.0)).collect();
        assert_eq!(waits, [secs(1), secs(2), secs(4), secs(8), secs(16), secs(16), secs(16)]);
    }

    #[test]
    fn jitter_stays_inside_its_band() {
        let backoff = Backoff::default();
        assert_eq!(backoff.delay(3, 1.0), Duration::from_millis(4800));
        assert_eq!(backoff.delay(3, -1.0), Duration::from_millis(3200));
        assert_eq!(backoff.delay(3, 50.0), Duration::from_millis(4800), "a wild spread is clamped");
    }

    #[test]
    fn a_huge_attempt_number_does_not_overflow() {
        assert_eq!(Backoff::default().delay(u32::MAX, 0.0), secs(16));
    }
}
