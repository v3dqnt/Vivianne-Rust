//! One writer per conversation. A run holds the conversation's lease while it works; a second process that tries to take it
//! is refused instead of loading the same history, appending its own turn, and overwriting the first one's on save.
//! Port of the original persistence layer's "only one writer per session" rule.
//!
//! The lease is an operating-system file lock on `<id>.lock` beside the thread files. The system lets go of it when the
//! process ends, however it ends, so a crash never leaves a conversation locked.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

/// Why a lease was not given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseError {
    /// Another Vivianne process is writing this conversation (its process id, when it said).
    Held { by: Option<u32> },
    /// The lock file could not be made or locked.
    Io(String),
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { by: Some(pid) } => write!(
                f,
                "this conversation is being written by another Vivianne (process {pid}); wait for it to finish"
            ),
            Self::Held { by: None } => write!(
                f,
                "this conversation is being written by another Vivianne; wait for it to finish"
            ),
            Self::Io(why) => write!(f, "could not take the conversation's lease: {why}"),
        }
    }
}

impl std::error::Error for LeaseError {}

/// The right to write one conversation. Dropping it gives the right up.
#[derive(Debug)]
pub struct Lease {
    file: File,
    path: PathBuf,
}

impl Lease {
    /// Takes the lease for `id` in `dir`, without waiting.
    pub fn take(dir: &Path, id: &str) -> Result<Self, LeaseError> {
        std::fs::create_dir_all(dir).map_err(|e| LeaseError::Io(e.to_string()))?;
        let path = dir.join(format!("{id}.lock"));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| LeaseError::Io(e.to_string()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut said = String::new();
                let _ = file.read_to_string(&mut said);
                return Err(LeaseError::Held {
                    by: said.trim().parse().ok(),
                });
            }
            Err(TryLockError::Error(e)) => return Err(LeaseError::Io(e.to_string())),
        }
        // Who holds it, for the one that is refused. Best effort: the lock, not this text, is what protects.
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();
        Ok(Self { file, path })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        // Gone with the lease, unless another process took it in between (then the removal fails or is theirs to redo).
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        std::env::temp_dir().join(format!("viv-lease-{}", uuid::Uuid::new_v4().simple()))
    }

    #[test]
    fn a_second_taker_is_refused_while_the_first_holds_it_and_may_take_it_after() {
        let d = dir();
        let first = Lease::take(&d, "thread-1").expect("first");
        match Lease::take(&d, "thread-1") {
            // On Windows the lock also keeps the file from being read, so who holds it is not always known.
            Err(LeaseError::Held { by }) => {
                assert!(by.is_none() || by == Some(std::process::id()), "{by:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        drop(first);
        assert!(Lease::take(&d, "thread-1").is_ok(), "released when dropped");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn different_conversations_do_not_block_each_other() {
        let d = dir();
        let (a, b) = (Lease::take(&d, "a"), Lease::take(&d, "b"));
        assert!(a.is_ok() && b.is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_refusal_reads_in_words_the_person_can_act_on() {
        let text = LeaseError::Held { by: Some(42) }.to_string();
        assert!(text.contains("another Vivianne") && text.contains("42") && text.contains("wait"));
    }
}
