//! What the agent loop adds to a conversation beside the person's words, before each model call: the workspace's
//! instruction files, the clock, the terminal's location. The rules live in `vivianne-context`; this file only decides
//! where in the history each message goes and reads the machine (the clock, the disk, tmux).
//!
//! Every piece is a durable message, so it is saved with the thread and replayed on resume, and an unchanged piece is
//! never said twice.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use vivianne_context::instructions::files::{Fs, LocalFs, Probe};
use vivianne_context::instructions::{Config as InstructionConfig, Engine, Resolved};
use vivianne_context::time::{self, Clock};
use vivianne_context::tmux::{self, Runner};
use vivianne_core::Message;

/// The disk, with one Vivianne addition: a single-line `@relative/path` in an instruction file is replaced by that file,
/// one level deep, so a `CLAUDE.md` that only says `@AGENTS.md` is not a second copy of the rules.
#[derive(Debug, Clone, Copy, Default)]
struct ImportingFs;

impl Fs for ImportingFs {
    fn probe(&self, path: &Path) -> Probe {
        LocalFs.probe(path)
    }

    fn marker_exists(&self, path: &Path) -> std::io::Result<bool> {
        LocalFs.marker_exists(path)
    }

    fn read_text(&self, path: &Path, max_bytes: u64) -> Option<String> {
        let raw = LocalFs.read_text(path, max_bytes)?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut out = String::with_capacity(raw.len());
        for line in raw.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('@')
                && !rest.is_empty()
                && !rest.contains(char::is_whitespace)
                && let Some(imported) = LocalFs.read_text(&dir.join(rest), max_bytes)
            {
                out.push_str(&imported);
                out.push('\n');
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        Some(out)
    }
}

/// Runs `tmux` on this machine.
struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The paths a tool call names, so a folder it reaches can bring its own instructions.
#[must_use]
pub fn touched_paths(arguments: &serde_json::Value) -> Vec<String> {
    let Some(map) = arguments.as_object() else {
        return Vec::new();
    };
    ["path", "file_path", "filePath", "file", "directory"]
        .iter()
        .filter_map(|key| map.get(*key)?.as_str())
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The context of one conversation.
pub struct WorkspaceContext {
    cwd: PathBuf,
    instructions: Engine,
    clock: Option<Clock>,
    tmux: bool,
    /// The zones the client reported for the open turn (a browser or the machine's own).
    zones: Mutex<Vec<String>>,
}

impl WorkspaceContext {
    /// Instructions from the files around `cwd`, the clock on the machine's zone, tmux when this process runs in it.
    #[must_use]
    pub fn new(cwd: &Path) -> Self {
        let resolved = Resolved::from_env(InstructionConfig::default());
        Self {
            cwd: cwd.to_path_buf(),
            instructions: Engine::new(resolved),
            clock: Clock::new(&time::Config::default()).ok(),
            tmux: std::env::var_os("TMUX_PANE").is_some(),
            zones: Mutex::new(Vec::new()),
        }
    }

    /// Tells the clock which zone the client is in, so unqualified dates in the request mean that zone's.
    pub fn report_zones(&self, zones: Vec<String>) {
        *self.zones.lock().unwrap_or_else(PoisonError::into_inner) = zones;
    }

    /// Adds what is due before model call number `step` (counted from 1) of the open turn. `touched` are the paths tools
    /// used since the last call. Never fails a turn: a probe that cannot be answered adds nothing.
    pub fn prepare(&self, history: &mut Vec<Message>, step: usize, touched: &[String]) {
        let now = now_ms();
        let typed = history.iter().rposition(Message::is_typed);
        if step == 1
            && let Some(i) = typed
            && history[i].at.is_none()
        {
            history[i].at = Some(now);
        }
        let turn = time::turn_of(history);
        if step == 1
            && self.tmux
            && let Some(i) = typed
            && let Some(location) = tmux::query(&SystemRunner, std::process::id())
            && let Some(reading) =
                tmux::reading(history, &location, turn, now, time::DEFAULT_REFRESH_MS)
        {
            // Before the person's message, so the message stays the last thing said.
            history.insert(i, reading);
        }
        let mut added = Vec::new();
        if let Ok(Some(message)) =
            self.instructions
                .compose(history, &self.cwd, touched, &ImportingFs)
        {
            added.push(message);
        }
        if let Some(clock) = &self.clock {
            let zones = self
                .zones
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let now_signed = i64::try_from(now).unwrap_or(i64::MAX);
            if let Ok(Some(message)) = clock.reading(history, now_signed, turn, step, &zones) {
                added.push(message);
            }
        }
        if added.is_empty() {
            return;
        }
        // On the first step they go right after what the person typed; later they follow the newest tool result.
        let at = match (step, history.iter().rposition(Message::is_typed)) {
            (1, Some(i)) => i + 1,
            _ => history.len(),
        };
        for (offset, message) in added.into_iter().enumerate() {
            history.insert(at + offset, message);
        }
    }
}

impl std::fmt::Debug for WorkspaceContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceContext")
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vivianne_core::Role;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-wc-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".git")).expect("dir");
        dir
    }

    #[test]
    fn the_first_step_puts_instructions_and_time_after_what_the_person_typed() {
        let dir = temp("order");
        std::fs::write(dir.join("AGENTS.md"), "BE TERSE").expect("write");
        let context = WorkspaceContext::new(&dir);
        let mut history = vec![Message::new(Role::User, "hello")];
        context.prepare(&mut history, 1, &[]);
        assert!(history[0].is_typed() && history[0].at.is_some());
        assert!(history[1].text.contains("BE TERSE"));
        assert!(history.iter().skip(1).all(|m| m.context.is_some()));
        // Nothing changed, nothing new is said on the next step.
        let before = history.len();
        context.prepare(&mut history, 2, &[]);
        assert_eq!(history.len(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_a_tool_reaches_brings_its_instructions_after_the_tool_result() {
        let dir = temp("nested");
        let context = WorkspaceContext::new(&dir);
        let mut history = vec![Message::new(Role::User, "go")];
        context.prepare(&mut history, 1, &[]);
        std::fs::create_dir_all(dir.join("sub")).expect("sub");
        std::fs::write(dir.join("sub/AGENTS.md"), "SUB RULES").expect("write");
        history.push(Message::new(Role::Assistant, "reading"));
        context.prepare(&mut history, 2, &["sub/x.rs".to_owned()]);
        assert!(history.last().is_some_and(|m| m.text.contains("SUB RULES")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_claude_md_that_only_imports_agents_md_is_said_once() {
        let dir = temp("import");
        std::fs::write(dir.join("AGENTS.md"), "ONE VOICE").expect("write");
        std::fs::write(dir.join("CLAUDE.md"), "@AGENTS.md\n").expect("write");
        let context = WorkspaceContext::new(&dir);
        let mut history = vec![Message::new(Role::User, "hi")];
        context.prepare(&mut history, 1, &[]);
        let said: usize = history
            .iter()
            .map(|m| m.text.matches("ONE VOICE").count())
            .sum();
        assert_eq!(said, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_come_from_the_usual_argument_names_and_nothing_else() {
        use serde_json::json;
        assert_eq!(
            touched_paths(&json!({"path":"a/b.rs","content":"x"})),
            ["a/b.rs"]
        );
        assert_eq!(touched_paths(&json!({"file_path":"c.rs"})), ["c.rs"]);
        assert!(touched_paths(&json!("not an object")).is_empty());
        assert!(touched_paths(&json!({"path":""})).is_empty());
        assert!(touched_paths(&json!({"command":"ls"})).is_empty());
    }
}
