//! Telling a dead link from a quiet one. A ping goes out after five seconds of silence; the link is given up on
//! when a ping has gone unanswered for ten, or when the first snapshot has not arrived a minute after connecting.
//! Any message at all counts as a reply, so a busy link is never pinged. Pure state, tested with a made-up clock.

use std::time::{Duration, Instant};

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
pub const INITIAL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(60);

/// What the connection should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Ping,
    Expired,
}

#[derive(Debug, Clone)]
pub struct Health {
    connected_at: Instant,
    last_heard: Instant,
    ping_sent: Option<Instant>,
    ready: bool,
}

impl Health {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            connected_at: now,
            last_heard: now,
            ping_sent: None,
            ready: false,
        }
    }

    /// Something arrived from the other side.
    pub fn heard(&mut self, now: Instant) {
        self.last_heard = now;
        self.ping_sent = None;
    }

    /// The first snapshot arrived: the machine is usable.
    pub fn became_ready(&mut self) {
        self.ready = true;
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn ping_sent(&mut self, now: Instant) {
        self.ping_sent = Some(now);
    }

    #[must_use]
    pub fn check(&self, now: Instant) -> Action {
        if !self.ready && now.duration_since(self.connected_at) >= INITIAL_SNAPSHOT_TIMEOUT {
            return Action::Expired;
        }
        if let Some(sent) = self.ping_sent {
            return if now.duration_since(sent) >= HEARTBEAT_TIMEOUT {
                Action::Expired
            } else {
                Action::None
            };
        }
        if now.duration_since(self.last_heard) >= HEARTBEAT_INTERVAL {
            return Action::Ping;
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_link_is_pinged_after_five_seconds_and_expires_when_the_ping_goes_unanswered() {
        let t0 = Instant::now();
        let mut h = Health::new(t0);
        h.became_ready();
        assert_eq!(h.check(t0 + Duration::from_secs(4)), Action::None);
        assert_eq!(h.check(t0 + Duration::from_secs(5)), Action::Ping);
        h.ping_sent(t0 + Duration::from_secs(5));
        assert_eq!(h.check(t0 + Duration::from_secs(14)), Action::None);
        assert_eq!(h.check(t0 + Duration::from_secs(15)), Action::Expired);
    }

    #[test]
    fn any_message_counts_as_a_reply_so_a_busy_link_is_never_pinged() {
        let t0 = Instant::now();
        let mut h = Health::new(t0);
        h.became_ready();
        h.ping_sent(t0 + Duration::from_secs(5));
        h.heard(t0 + Duration::from_secs(8));
        assert_eq!(
            h.check(t0 + Duration::from_secs(20)),
            Action::Ping,
            "quiet again 5 s after the reply, not expired"
        );
        h.heard(t0 + Duration::from_secs(19));
        assert_eq!(h.check(t0 + Duration::from_secs(20)), Action::None);
    }

    #[test]
    fn a_connection_that_never_sends_its_first_snapshot_is_given_up_on_after_a_minute() {
        let t0 = Instant::now();
        let mut h = Health::new(t0);
        h.heard(t0 + Duration::from_secs(59));
        assert_eq!(h.check(t0 + Duration::from_secs(59)), Action::None);
        assert_eq!(h.check(t0 + Duration::from_secs(60)), Action::Expired);
    }
}
