//! Persistent terminals: shells whose state survives between tool calls. A toolbox owns its own, and they end with it.
//!
//! Port of `@deepseek-ai/dsh-terminal`, `dsh-terminal-bash` and `experimental/tool-terminal`.

mod session;
mod tools;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use vivianne_sandbox::confine;
use vivianne_sandbox::policy::{Mode, Policy};

use crate::confinement::Confinement;
use crate::jobs::Jobs;
use session::Session;

pub use tools::{Kind, TerminalTool};

/// Terminals one toolbox may hold at once.
const MAX_SESSIONS: usize = 8;

#[derive(Debug, Clone)]
struct Record {
    id: String,
    name: Option<String>,
    session: Arc<Session>,
}

#[derive(Debug, Default)]
struct Table {
    next: u64,
    records: Vec<Record>,
}

/// One toolbox's terminals. Clones share them.
#[derive(Debug, Clone)]
pub struct Terminals {
    root: PathBuf,
    confinement: Confinement,
    jobs: Jobs,
    table: Arc<Mutex<Table>>,
}

fn shell_argv() -> Vec<String> {
    if cfg!(windows) {
        ["powershell", "-NoLogo", "-NoProfile"]
            .map(str::to_owned)
            .to_vec()
    } else {
        ["bash", "--noprofile", "--norc", "-i"]
            .map(str::to_owned)
            .to_vec()
    }
}

impl Terminals {
    #[must_use]
    pub fn new(root: &Path, confinement: Confinement, jobs: Jobs) -> Self {
        Self {
            root: root.to_path_buf(),
            confinement,
            jobs,
            table: Arc::default(),
        }
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The shell, wrapped for the standing mode; a mode that cannot be enforced refuses it.
    fn argv(&self) -> Result<Vec<String>, String> {
        let mode = self.confinement.get();
        if mode == Mode::DangerFullAccess {
            return Ok(shell_argv());
        }
        let root = std::path::absolute(&self.root).map_err(|e| e.to_string())?;
        let policy = Policy::new(mode, &root, &[]).map_err(|e| e.to_string())?;
        confine::confine(&shell_argv(), &policy)
            .map(|confined| confined.argv)
            .map_err(|e| e.to_string())
    }

    /// A folder inside the project: where a terminal may start.
    fn folder(&self, cwd: Option<&str>) -> Result<PathBuf, String> {
        let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
            return Ok(self.root.clone());
        };
        let workspace = crate::simple::Workspace::new(&self.root);
        let folder = workspace.resolve(cwd).map_err(|e| e.to_string())?;
        if folder.is_dir() {
            Ok(folder)
        } else {
            Err(format!("`{cwd}` is not a folder"))
        }
    }

    fn open(&self, name: Option<&str>, cwd: Option<&str>) -> Result<Record, String> {
        if name.is_some_and(str::is_empty) {
            return Err("terminal name must be non-empty".to_owned());
        }
        let argv = self.argv()?;
        let folder = self.folder(cwd)?;
        let mut table = self.table();
        if let Some(name) = name
            && table
                .records
                .iter()
                .any(|r| r.name.as_deref() == Some(name))
        {
            return Err(format!("terminal name \"{name}\" already exists"));
        }
        if table.records.len() >= MAX_SESSIONS {
            return Err(format!(
                "terminal limit reached ({MAX_SESSIONS}); close one with terminal_close"
            ));
        }
        let session = Arc::new(Session::start(&argv, &folder)?);
        table.next += 1;
        let record = Record {
            id: format!("term-{}", table.next),
            name: name.map(str::to_owned),
            session,
        };
        table.records.push(record.clone());
        Ok(record)
    }

    fn get(&self, id: &str) -> Result<Record, String> {
        self.table()
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| format!("unknown terminal session {id}"))
    }

    fn close(&self, id: &str) -> Result<(), String> {
        let mut table = self.table();
        let at = table
            .records
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| format!("unknown terminal session {id}"))?;
        // Dropped here, or when a send still holding it ends.
        let record = table.records.remove(at);
        record.session.write(b"\x03exit\r".to_vec());
        Ok(())
    }

    fn list(&self) -> Vec<Record> {
        self.table().records.clone()
    }
}
