//! A tool result too big for the model (R3.7). The whole of it is kept in a file under `~/.viv/agent/tool-results/`,
//! and the model is given a preview and a handle it can read from with `read_tool_result`, a piece at a time. Nothing
//! is lost and the context window is not filled by one huge `grep` or page.
//!
//! The limit is 64 KiB by default (`max_tool_result_bytes`, never under 1 KiB). The kept files are capped too: the
//! oldest go when there are more than [`KEEP_FILES`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::builtin::{Tool, ToolResult};

pub const DEFAULT_LIMIT: usize = 65_536;
pub const MIN_LIMIT: usize = 1024;
/// How much of a stored result the model sees at once, when it does not say.
const PREVIEW_BYTES: usize = 2048;
/// Stored results kept; older ones are removed.
pub const KEEP_FILES: usize = 200;

pub const TOOL_NAME: &str = "read_tool_result";

static LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_LIMIT);

/// Sets the limit (from configuration). Values under the minimum are raised to it.
pub fn set_limit(bytes: usize) {
    LIMIT.store(bytes.max(MIN_LIMIT), Ordering::Relaxed);
}

#[must_use]
pub fn limit() -> usize {
    LIMIT.load(Ordering::Relaxed)
}

fn dir() -> PathBuf {
    crate::agent::system::paths::viv_home_path(&["agent", "tool-results"])
}

/// The largest index at most `at` that is the start of a character, so a slice never cuts one in half.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Keeps `content` under `dir` and returns its handle. Written beside the final name and renamed (R3.4).
fn store_in(dir: &Path, content: &str) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let tmp = dir.join(format!("{id}.tmp"));
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(format!("{id}.txt"))).map_err(|e| e.to_string())?;
    prune(dir);
    Ok(id)
}

/// Removes the oldest stored results beyond [`KEEP_FILES`].
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= KEEP_FILES {
        return;
    }
    files.sort();
    for (_, path) in &files[..files.len() - KEEP_FILES] {
        // A result already gone is what was wanted.
        let _ = std::fs::remove_file(path);
    }
}

/// Reads a piece of a stored result: up to `max` bytes from `offset`.
fn read_in(dir: &Path, id: &str, offset: usize, max: usize) -> Result<String, String> {
    if !valid_id(id) {
        return Err("That is not a tool result id.".to_owned());
    }
    let text = std::fs::read_to_string(dir.join(format!("{id}.txt")))
        .map_err(|_| "No stored result has that id (it may have been removed).".to_owned())?;
    let total = text.len();
    if offset >= total {
        return Ok(format!(
            "[End of result: it has {total} bytes and the offset is {offset}.]"
        ));
    }
    let from = floor_boundary(&text, offset);
    let to = floor_boundary(&text, (from + max.max(1)).min(total));
    // A piece must always move forward, even when one character is wider than `max`.
    let to = if to <= from {
        text[from..]
            .chars()
            .next()
            .map_or(total, |c| from + c.len_utf8())
    } else {
        to
    };
    let mut out = text[from..to].to_owned();
    if to < total {
        out.push_str(&format!(
            "\n[bytes {from} to {to} of {total}. Read on with offset {to}.]"
        ));
    } else {
        out.push_str(&format!("\n[bytes {from} to {total} of {total}: the end.]"));
    }
    Ok(out)
}

/// Applies the limit to a result: a short one is returned as it is, a long one is stored and replaced by a preview
/// and its handle. A failure to store leaves the start of the result and says so; the model never gets the whole.
#[must_use]
pub fn cap(content: String) -> String {
    cap_in(&dir(), content, limit())
}

fn cap_in(dir: &Path, content: String, limit: usize) -> String {
    if content.len() <= limit {
        return content;
    }
    let total = content.len();
    let preview = &content[..floor_boundary(&content, PREVIEW_BYTES.min(limit))];
    match store_in(dir, &content) {
        Ok(id) => format!(
            "{preview}\n\n[Result too large: {total} bytes, only the first {} are shown. It is saved as tool result {id}; read more with {TOOL_NAME} (id, offset, limit).]",
            preview.len()
        ),
        Err(why) => {
            let cut = &content[..floor_boundary(&content, limit)];
            format!(
                "{cut}\n\n[Result too large: {total} bytes, cut to {} because it could not be saved ({why}).]",
                cut.len()
            )
        }
    }
}

/// The tool that reads a stored result.
#[derive(Debug, Default)]
pub struct ReadToolResult;

#[async_trait]
impl Tool for ReadToolResult {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": TOOL_NAME,
            "description": "Read more of a tool result that was too large to show. Pass the id from the note at the end of that result, and an offset to continue from.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "The tool result id."},
                    "offset": {"type": "integer", "description": "Byte offset to read from. Default 0."},
                    "limit": {"type": "integer", "description": "Most bytes to read. Default and most: the result limit."}
                },
                "required": ["id"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let id = args["id"].as_str().unwrap_or_default();
        let offset = args["offset"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let max = args["limit"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(usize::MAX)
            .min(limit());
        match read_in(&dir(), id, offset, max) {
            Ok(content) => ToolResult {
                content,
                is_error: false,
                details: None,
            },
            Err(content) => ToolResult {
                content,
                is_error: true,
                details: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("viv-results-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn a_short_result_is_untouched_and_a_long_one_becomes_a_preview_with_a_handle() {
        let dir = temp();
        assert_eq!(cap_in(&dir, "small".to_owned(), 1024), "small");
        let big = "line\n".repeat(1000);
        let shown = cap_in(&dir, big.clone(), 1024);
        assert!(shown.len() < big.len() && shown.contains("Result too large: 5000 bytes"));
        let id = shown
            .split("tool result ")
            .nth(1)
            .and_then(|s| s.split(';').next())
            .expect("a handle")
            .to_owned();
        assert!(valid_id(&id));
        // The whole result is kept, and reading in pieces returns all of it.
        let mut got = String::new();
        let mut offset = 0;
        loop {
            let piece = read_in(&dir, &id, offset, 1500).expect("reads");
            let (body, note) = piece.rsplit_once("\n[").expect("a note");
            got.push_str(body);
            if note.contains("the end") {
                break;
            }
            offset = note
                .rsplit("offset ")
                .next()
                .and_then(|n| n.trim_end_matches(".]").parse().ok())
                .expect("next offset");
        }
        assert_eq!(got, big);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_character_is_never_cut_in_half_and_a_piece_always_moves_forward() {
        let dir = temp();
        let text = "é".repeat(2000); // two bytes each
        let shown = cap_in(&dir, text, 1024);
        assert!(shown.is_char_boundary(shown.len()));
        let id = shown
            .split("tool result ")
            .nth(1)
            .and_then(|s| s.split(';').next())
            .expect("handle")
            .to_owned();
        // A limit of 1 byte still returns one whole character.
        let piece = read_in(&dir, &id, 0, 1).expect("reads");
        assert!(piece.starts_with('é'));
        // An offset in the middle of a character starts at that character.
        assert!(read_in(&dir, &id, 1, 4).expect("reads").starts_with('é'));
        assert!(
            read_in(&dir, &id, 99_999, 10)
                .expect("reads")
                .contains("End of result")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_real_handle_reads_and_a_path_cannot_be_smuggled_in() {
        let dir = temp();
        for bad in ["../x", "", "abc", "g".repeat(32).as_str(), "..\\..\\secret"] {
            assert!(read_in(&dir, bad, 0, 10).is_err(), "{bad}");
        }
        assert!(
            read_in(&dir, &"a".repeat(32), 0, 10)
                .unwrap_err()
                .contains("No stored result")
        );
    }

    #[test]
    fn the_limit_never_goes_under_one_kilobyte_and_old_results_are_pruned() {
        let before = limit();
        set_limit(10);
        assert_eq!(limit(), MIN_LIMIT);
        set_limit(before);
        let dir = temp();
        for _ in 0..KEEP_FILES + 5 {
            store_in(&dir, "x").expect("stores");
        }
        let kept = std::fs::read_dir(&dir)
            .expect("lists")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .count();
        assert_eq!(kept, KEEP_FILES);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
