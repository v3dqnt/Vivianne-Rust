//! What git says about the project folder. Runs the `git` program; no UI types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the repository root, with forward slashes.
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

impl ChangeKind {
    /// The letter shown beside the file.
    #[must_use]
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
        }
    }
}

/// The repository's state, read once.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub branch: Option<String>,
    pub last_commit: Option<String>,
    pub changes: Vec<Change>,
    /// Absolute path of the repository root.
    pub top: Option<PathBuf>,
}

impl Snapshot {
    #[must_use]
    pub fn read(dir: &Path) -> Self {
        let Some(top) = run(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from) else {
            return Self::default();
        };
        Self {
            branch: run(dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
            last_commit: run(dir, &["log", "-1", "--pretty=%h %s"]),
            changes: run_raw(dir, &["status", "--porcelain"])
                .map(|out| parse_status(&out))
                .unwrap_or_default(),
            top: Some(top),
        }
    }

    /// Changes keyed by absolute path, for marking rows in the file tree.
    #[must_use]
    pub fn by_path(&self) -> HashMap<PathBuf, ChangeKind> {
        let Some(top) = &self.top else {
            return HashMap::new();
        };
        self.changes
            .iter()
            .map(|c| (top.join(&c.path), c.kind))
            .collect()
    }
}

/// Reads `git status --porcelain` lines (`XY path`, or `XY old -> new`).
#[must_use]
pub fn parse_status(output: &str) -> Vec<Change> {
    output.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Change> {
    let (code, rest) = (line.get(..2)?, line.get(3..)?);
    let path = rest.rsplit(" -> ").next()?.trim_matches('"').to_owned();
    let mut letters = code.chars();
    let (index, worktree) = (letters.next()?, letters.next()?);
    let kind = match (index, worktree) {
        ('?', _) => ChangeKind::Untracked,
        ('R', _) | (_, 'R') => ChangeKind::Renamed,
        ('D', _) | (_, 'D') => ChangeKind::Deleted,
        ('A', _) => ChangeKind::Added,
        _ => ChangeKind::Modified,
    };
    Some(Change { path, kind })
}

fn run(dir: &Path, args: &[&str]) -> Option<String> {
    run_raw(dir, args)
        .map(|out| out.trim().to_owned())
        .filter(|out| !out.is_empty())
}

fn run_raw(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_become_changes() {
        let changes =
            parse_status(" M src/a.rs\nA  src/b.rs\n?? notes.md\n D old.rs\nR  x.rs -> y.rs\n");
        let kinds: Vec<_> = changes.iter().map(|c| (c.path.as_str(), c.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("src/a.rs", ChangeKind::Modified),
                ("src/b.rs", ChangeKind::Added),
                ("notes.md", ChangeKind::Untracked),
                ("old.rs", ChangeKind::Deleted),
                ("y.rs", ChangeKind::Renamed),
            ]
        );
    }

    #[test]
    fn a_folder_that_is_not_a_repository_reads_as_empty() {
        let dir = std::env::temp_dir();
        let snapshot = Snapshot::read(&dir);
        assert!(snapshot.changes.is_empty() || snapshot.top.is_some());
    }
}
