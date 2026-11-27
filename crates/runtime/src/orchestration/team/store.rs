//! One file per conversation, `~/.viv/agent/teams/<thread>.json`, changed under an OS file lock and replaced whole.

use std::path::PathBuf;

use super::board::{Board, TeamError, error};

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

fn storage(what: impl std::fmt::Display) -> TeamError {
    error(
        "TEAM_STORAGE",
        format!("the task board could not be kept: {what}"),
    )
}

impl Store {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The person's own boards.
    #[must_use]
    pub fn standard() -> Self {
        // Without a home there is nowhere to keep one; the first change says so.
        Self::new(
            vivianne_ai::paths::viv_agent_dir()
                .map_or_else(|_| PathBuf::new(), |d| d.join("teams")),
        )
    }

    /// A thread id is used as a file name, so only the characters an id has are taken.
    fn path(&self, thread: &str) -> Result<PathBuf, TeamError> {
        let plain = !thread.is_empty()
            && thread.len() <= 64
            && thread
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !plain || self.dir.as_os_str().is_empty() {
            return Err(storage(format!("not a conversation id: {thread}")));
        }
        Ok(self.dir.join(format!("{thread}.json")))
    }

    pub fn read(&self, thread: &str) -> Result<Board, TeamError> {
        match std::fs::read_to_string(self.path(thread)?) {
            // A file that cannot be understood is an error, never an empty board written over it.
            Ok(text) => serde_json::from_str(&text).map_err(storage),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Board::default()),
            Err(e) => Err(storage(e)),
        }
    }

    /// Runs `change` on the board while holding its lock, and keeps the result if the change succeeds.
    pub fn change<T>(
        &self,
        thread: &str,
        change: impl FnOnce(&mut Board) -> Result<T, TeamError>,
    ) -> Result<T, TeamError> {
        let path = self.path(thread)?;
        std::fs::create_dir_all(&self.dir).map_err(storage)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(storage)?;
        lock.lock().map_err(storage)?;
        let mut board = self.read(thread)?;
        let value = change(&mut board)?;
        let temporary = path.with_extension("tmp");
        let text = serde_json::to_string_pretty(&board).map_err(storage)?;
        std::fs::write(&temporary, text).map_err(storage)?;
        std::fs::rename(&temporary, &path).map_err(storage)?;
        Ok(value)
    }
}
