//! A short, in-memory log that every crate can write to and the Debug pane reads: what the background parts are
//! doing (machines connecting, servers starting, runs failing). It is not a file and nothing here leaves the
//! process; it holds the last few hundred lines and forgets the rest.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

/// Most lines kept.
pub const CAPACITY: usize = 500;

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Seconds since 1970.
    pub at: u64,
    /// What part wrote it: `remote`, `mcp`, `run`, `app`.
    pub category: &'static str,
    pub text: String,
}

static LOG: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());

/// Adds a line. The oldest goes when the log is full.
pub fn log(category: &'static str, text: impl Into<String>) {
    let at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut log = LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if log.len() >= CAPACITY {
        log.pop_front();
    }
    log.push_back(Entry {
        at,
        category,
        text: text.into(),
    });
}

/// The lines, newest first.
#[must_use]
pub fn entries() -> Vec<Entry> {
    LOG.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .cloned()
        .collect()
}

/// Empties the log.
pub fn clear() {
    LOG.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_the_newest_lines_newest_first_and_forgets_the_oldest() {
        clear();
        for i in 0..CAPACITY + 5 {
            log("test", format!("line {i}"));
        }
        let all = entries();
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].text, format!("line {}", CAPACITY + 4));
        assert_eq!(all[CAPACITY - 1].text, "line 5");
        clear();
        assert!(entries().is_empty());
    }
}
