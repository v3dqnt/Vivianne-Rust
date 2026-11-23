//! What a run changed, per file with line counts, and each file's comparison. Port of the original `workspace-changes`:
//! git snapshots of the working tree at the start and end of a run are diffed, and every file a file tool edits is copied
//! whole before its first edit and again at the end, which covers the files git does not (ignored ones, ones outside the
//! repository, and every edit when there is no repository).
//!
//! Not the same as `checkpoints`: those are hidden refs kept for rolling back; these are private snapshots that leave the
//! repository untouched and are forgotten with the conversation.

mod capture;
mod compare;
mod git;
mod numstat;
mod paths;
mod recorder;
mod types;

pub use recorder::Recorder;
pub use types::{ChangedFile, ChangesSummary, FileDiff, Hunk, Limits, SnapshotIds};

/// The tools that write a file, and the arguments that name the file they write.
const EDITING_TOOLS: [&str; 4] = ["write", "edit", "str_replace_editor", "lsp_edit"];
const PATH_ARGUMENTS: [&str; 3] = ["path", "file_path", "filePath"];

/// The paths a tool call is about to change, when it is one that writes a file.
#[must_use]
pub fn edited_paths(tool: &str, arguments: &serde_json::Value) -> Vec<String> {
    if !EDITING_TOOLS.contains(&tool) {
        return Vec::new();
    }
    PATH_ARGUMENTS
        .iter()
        .filter_map(|key| arguments.get(*key)?.as_str())
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests;
