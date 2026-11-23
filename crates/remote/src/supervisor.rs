//! Keeping a machine connected: when to try again, and how long to wait. This is the state machine only (no I/O),
//! so every rule can be tested with a made-up clock. The numbers are herdr's.
//!
//! A drop is retried after 500 ms, doubling up to two minutes (thirty seconds for this computer). A connection
//! has to stay healthy for a minute before a later drop gets a fast retry again. A machine that needs the person
//! (a changed host key, a key that is not accepted) is shown as `Attention` and still retried every thirty
//! seconds, so it recovers by itself once fixed. After the computer wakes, retries are quick for a while.

use std::time::{Duration, Instant};

pub const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(500);
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(120);
pub const MAX_LOCAL_RETRY_DELAY: Duration = Duration::from_secs(30);
pub const STABLE_CONNECTION_PERIOD: Duration = Duration::from_secs(60);
pub const WAKE_RECOVERY_PERIOD: Duration = Duration::from_secs(30);
pub const WAKE_RETRY_DELAY: Duration = Duration::from_secs(2);
pub const ATTENTION_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Where a machine's connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Online,
    Reconnecting,
    /// Needs the person (see [`crate::ssh::Failure::needs_attention`]).
    Attention,
    Disabled,
}

/// The wait before retry number `attempt` (1 is the first retry): 500 ms doubling, to `max`.
#[must_use]
pub fn retry_delay(attempt: u32, max: Duration) -> Duration {
    let doublings = attempt.saturating_sub(1).min(8);
    (INITIAL_RETRY_DELAY * 2u32.pow(doublings)).min(max)
}

/// The computer's power changing, as the window system reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
}

/// One machine's retry state.
#[derive(Debug, Clone)]
pub struct Supervisor {
    local: bool,
    status: Status,
    attempts: u32,
    ready_since: Option<Instant>,
    wake_until: Option<Instant>,
    suspended: bool,
    /// When the next attempt may start, if one is waiting.
    next_attempt: Option<Instant>,
}

impl Supervisor {
    #[must_use]
    pub fn new(local: bool, enabled: bool) -> Self {
        Self {
            local,
            status: if enabled {
                Status::Connecting
            } else {
                Status::Disabled
            },
            attempts: 0,
            ready_since: None,
            wake_until: None,
            suspended: false,
            next_attempt: None,
        }
    }

    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    #[must_use]
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    #[must_use]
    pub fn next_attempt(&self) -> Option<Instant> {
        self.next_attempt
    }

    pub fn enable(&mut self, now: Instant) {
        if self.status == Status::Disabled {
            self.status = Status::Connecting;
            self.attempts = 0;
            self.next_attempt = Some(now);
        }
    }

    pub fn disable(&mut self) {
        self.status = Status::Disabled;
        self.next_attempt = None;
        self.ready_since = None;
    }

    /// A connection attempt began.
    pub fn attempt_started(&mut self) {
        self.next_attempt = None;
        if self.status != Status::Disabled && self.status != Status::Attention {
            self.status = if self.attempts == 0 {
                Status::Connecting
            } else {
                Status::Reconnecting
            };
        }
    }

    /// The handshake finished and the machine is usable.
    pub fn connected(&mut self, now: Instant) {
        self.status = Status::Online;
        self.ready_since = Some(now);
        self.next_attempt = None;
        // This computer's own server is retried from scratch each time; a remote keeps its count until the
        // connection has proven itself (see `dropped`).
        if self.local {
            self.attempts = 0;
        }
    }

    /// The connection ended or an attempt failed. `needs_attention` is the failure's kind; `now` is the clock.
    pub fn dropped(&mut self, now: Instant, needs_attention: bool) {
        if self.status == Status::Disabled {
            return;
        }
        if self
            .ready_since
            .is_some_and(|since| now.duration_since(since) >= STABLE_CONNECTION_PERIOD)
        {
            self.attempts = 0;
        }
        self.ready_since = None;
        self.attempts += 1;
        let (status, delay) = if needs_attention && !self.local {
            (Status::Attention, ATTENTION_RETRY_DELAY)
        } else {
            let max = if self.local {
                MAX_LOCAL_RETRY_DELAY
            } else {
                MAX_RETRY_DELAY
            };
            (Status::Reconnecting, self.delay_for(now, max))
        };
        self.status = status;
        self.next_attempt = (!self.suspended).then_some(now + delay);
    }

    fn delay_for(&self, now: Instant, max: Duration) -> Duration {
        if self.wake_until.is_some_and(|until| now < until) {
            return WAKE_RETRY_DELAY.min(retry_delay(self.attempts, max));
        }
        retry_delay(self.attempts, max)
    }

    /// The person asked to reconnect now (`machine reconnect`): the count starts over and the attempt is immediate.
    pub fn retry_now(&mut self, now: Instant) {
        if self.status == Status::Disabled {
            return;
        }
        self.attempts = 0;
        self.next_attempt = Some(now);
    }

    /// Whether an attempt should start at `now`.
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        !self.suspended && self.next_attempt.is_some_and(|at| now >= at)
    }

    pub fn power(&mut self, event: PowerEvent, now: Instant) {
        match event {
            PowerEvent::Suspend => {
                // Nothing new starts while asleep; what was in flight is the caller's to cancel.
                self.suspended = true;
                self.next_attempt = None;
            }
            PowerEvent::Resume => {
                self.suspended = false;
                self.wake_until = Some(now + WAKE_RECOVERY_PERIOD);
                if self.status != Status::Disabled && self.status != Status::Online {
                    // After waking, the old backoff means nothing: try at once.
                    self.next_attempt = Some(now);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_doubles_from_half_a_second_and_stops_at_the_cap() {
        let secs = |n| retry_delay(n, MAX_RETRY_DELAY).as_millis();
        assert_eq!(
            [secs(1), secs(2), secs(3), secs(4)],
            [500, 1000, 2000, 4000]
        );
        assert_eq!(secs(9), 120_000);
        assert_eq!(secs(50), 120_000);
        assert_eq!(
            retry_delay(50, MAX_LOCAL_RETRY_DELAY),
            MAX_LOCAL_RETRY_DELAY
        );
    }

    #[test]
    fn a_connection_must_stay_up_a_minute_before_its_next_drop_is_retried_quickly() {
        let t0 = Instant::now();
        let mut s = Supervisor::new(false, true);
        s.dropped(t0, false);
        s.dropped(t0, false);
        s.dropped(t0, false);
        assert_eq!(s.attempts(), 3);
        // Up for ten seconds only: the count carries on.
        s.connected(t0);
        s.dropped(t0 + Duration::from_secs(10), false);
        assert_eq!(s.attempts(), 4);
        // Up for a minute: the next drop starts over at the short delay.
        s.connected(t0 + Duration::from_secs(20));
        s.dropped(t0 + Duration::from_secs(85), false);
        assert_eq!(s.attempts(), 1);
        assert_eq!(
            s.next_attempt(),
            Some(t0 + Duration::from_secs(85) + INITIAL_RETRY_DELAY)
        );
        assert_eq!(s.status(), Status::Reconnecting);
    }

    #[test]
    fn a_machine_that_needs_the_person_is_marked_and_still_retried_every_thirty_seconds() {
        let t0 = Instant::now();
        let mut s = Supervisor::new(false, true);
        s.dropped(t0, true);
        assert_eq!(s.status(), Status::Attention);
        assert_eq!(s.next_attempt(), Some(t0 + ATTENTION_RETRY_DELAY));
        assert!(!s.due(t0) && s.due(t0 + ATTENTION_RETRY_DELAY));
        s.retry_now(t0);
        assert!(s.due(t0) && s.attempts() == 0);
    }

    #[test]
    fn a_disabled_machine_never_retries_and_enabling_starts_afresh() {
        let t0 = Instant::now();
        let mut s = Supervisor::new(false, false);
        assert_eq!(s.status(), Status::Disabled);
        s.dropped(t0, false);
        s.retry_now(t0);
        assert!(!s.due(t0 + Duration::from_secs(999)));
        s.enable(t0);
        assert!(s.due(t0) && s.status() == Status::Connecting);
    }

    #[test]
    fn nothing_starts_while_asleep_and_waking_retries_at_once_then_quickly() {
        let t0 = Instant::now();
        let mut s = Supervisor::new(false, true);
        s.dropped(t0, false);
        s.dropped(t0, false);
        s.dropped(t0, false);
        s.dropped(t0, false);
        s.power(PowerEvent::Suspend, t0);
        assert!(!s.due(t0 + Duration::from_secs(600)), "asleep");
        let wake = t0 + Duration::from_secs(700);
        s.power(PowerEvent::Resume, wake);
        assert!(s.due(wake), "tries at once after waking");
        s.attempt_started();
        s.dropped(wake, false);
        assert!(
            s.next_attempt().expect("scheduled") <= wake + WAKE_RETRY_DELAY,
            "quick during the recovery window"
        );
        let later = wake + Duration::from_secs(40);
        s.dropped(later, false);
        assert!(
            s.next_attempt().expect("scheduled") > later + WAKE_RETRY_DELAY,
            "the window has closed"
        );
    }

    #[test]
    fn this_computers_own_server_is_retried_from_scratch_and_capped_at_thirty_seconds() {
        let t0 = Instant::now();
        let mut s = Supervisor::new(true, true);
        for _ in 0..12 {
            s.dropped(t0, false);
        }
        assert_eq!(s.next_attempt(), Some(t0 + MAX_LOCAL_RETRY_DELAY));
        s.connected(t0);
        assert_eq!(s.attempts(), 0);
    }
}
