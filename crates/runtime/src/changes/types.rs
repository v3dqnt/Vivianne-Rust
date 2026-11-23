//! What a turn changed, as the recorder reports it: the files with their line counts, and one file's comparison.

use serde::{Deserialize, Serialize};

/// One file changed during a turn, with line counts from git or from the whole-file captures around its edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    /// Relative to the working folder, or an absolute path outside it.
    pub path: String,
    /// Sort key and label: the relative path inside the working folder, a `../` path for repository files above it, a
    /// `~` path under the home folder, otherwise the absolute path. Always slash-separated.
    pub display: String,
    /// Lines added; zero for a binary or oversized file.
    pub added: usize,
    /// Lines deleted; zero for a binary or oversized file.
    pub deleted: usize,
    /// Git reported it as binary, or a captured side holds a NUL byte.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
    /// A captured side was over the size cap: listed without counts or comparison.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub oversized: bool,
}

/// The git trees the two snapshots wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotIds {
    pub before: String,
    pub after: String,
}

/// Files changed during one turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesSummary {
    pub turn: usize,
    /// The working folder `path` values are relative to.
    pub cwd: String,
    /// In `display` order, at most the recorder's file cap.
    pub files: Vec<ChangedFile>,
    /// Every changed file, including those the cap left out.
    pub total: usize,
    pub added: usize,
    pub deleted: usize,
    /// Absent when no snapshot was taken (no repository, or no git).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotIds>,
}

/// One unified-diff hunk with three context lines; every line keeps its `+`, `-` or space prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<String>,
}

/// The comparison of one listed file's two sides, computed when asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FileDiff {
    Text {
        path: String,
        display: String,
        /// Whether the file existed at the start of the turn.
        before: bool,
        /// Whether it existed at the end.
        after: bool,
        hunks: Vec<Hunk>,
        /// The comparison ran out of time and every line is shown as replaced.
        coarse: bool,
    },
    /// A side git reported as binary or that holds a NUL byte: no lines are served.
    Binary { path: String, display: String },
    /// A side over the size cap: no lines are served.
    Oversized { path: String, display: String },
}

/// Bounds of the recorder. Every number must be positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Milliseconds one git command may run before the turn's record is abandoned.
    pub timeout_ms: u64,
    /// Bytes of git output kept per command.
    pub output_max_bytes: usize,
    /// Files one summary carries; `total` still counts them all.
    pub max_files: usize,
    /// Bytes a file may hold to be captured around an edit or read from a snapshot for its comparison.
    pub max_file_bytes: usize,
    /// Milliseconds a line comparison may run before it degrades to whole-file replacement.
    pub diff_timeout_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            output_max_bytes: 8 * 1024 * 1024,
            max_files: 500,
            max_file_bytes: 2 * 1024 * 1024,
            diff_timeout_ms: 100,
        }
    }
}

impl Limits {
    /// Names the first field that is zero.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("timeout_ms", self.timeout_ms),
            ("output_max_bytes", self.output_max_bytes as u64),
            ("max_files", self.max_files as u64),
            ("max_file_bytes", self.max_file_bytes as u64),
            ("diff_timeout_ms", self.diff_timeout_ms),
        ] {
            if value == 0 {
                return Err(format!("changes: {name} must be positive"));
            }
        }
        Ok(())
    }
}
