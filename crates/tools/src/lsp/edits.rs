// Copied from Vivianne's original tools (0901 lsp/edits.rs).
// Kept as close to the original as possible; do not restyle.
#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, clippy::todo, clippy::dbg_macro, unsafe_code, missing_debug_implementations)]

//! Applying what a language server sends back as edits: `WorkspaceEdit` (rename, code actions, server-initiated `workspace/applyEdit`) and
//! `TextEdit[]` (formatting), plus the formatting options a file asks for.
//!
//! All or nothing: every file's new content is worked out and every path checked against the workspace before anything is written, and each write
//! goes through the file-safety layer's atomic replace. A path outside the workspace refuses the whole edit.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::uri::{path_key, uri_to_path};

/// One planned change to the disk.
#[derive(Debug, PartialEq)]
enum Step {
    Write(PathBuf, String),
    Create(PathBuf),
    Rename(PathBuf, PathBuf),
    Delete(PathBuf),
}

/// Whether `path` is the workspace `root` or inside it: lexically (so `..` cannot walk out), and, for what exists, by real path (so a symlink cannot).
pub fn inside_root(path: &Path, root: &str) -> bool {
    let (p, r) = (path_key(&path.to_string_lossy()), path_key(root));
    if p != r && !p.starts_with(&format!("{}/", r.trim_end_matches('/'))) {
        return false;
    }
    // The nearest part that exists decides the real location.
    let mut existing = path;
    while !existing.exists() {
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return true,
        }
    }
    match (std::fs::canonicalize(existing), std::fs::canonicalize(root)) {
        (Ok(a), Ok(b)) => a.starts_with(b),
        _ => true,
    }
}

/// Byte offset of an LSP position (UTF-16 units) in `text`, clamped to the line's end the way servers expect.
fn offset_of(text: &str, line: u32, character: u32) -> usize {
    let mut start = 0usize;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let line_end = text[start..].find('\n').map(|i| start + i).unwrap_or(text.len());
    let body_end = if text[start..line_end].ends_with('\r') { line_end - 1 } else { line_end };
    let mut units = 0u32;
    for (i, c) in text[start..body_end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16() as u32;
    }
    body_end
}

/// `edits` (a `TextEdit[]`) applied to `text`. Edits are in terms of the original text and must not overlap.
pub fn apply_text_edits(text: &str, edits: &[Value]) -> Result<String, String> {
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for e in edits {
        let pos = |p: &str, k: &str| e.pointer(&format!("/range/{p}/{k}")).and_then(|v| v.as_u64()).map(|v| v as u32);
        let (Some(sl), Some(sc), Some(el), Some(ec)) = (pos("start", "line"), pos("start", "character"), pos("end", "line"), pos("end", "character")) else {
            return Err("an edit has no range".into());
        };
        // A snippet (`$1`, `${2:name}`) is for an editor that can place the cursor; written to a file it would be literal garbage.
        if e.get("insertTextFormat").and_then(|f| f.as_u64()) == Some(2) {
            return Err("the server sent a snippet edit, which can't be written to a file as is".into());
        }
        let new_text = e.get("newText").and_then(|t| t.as_str()).ok_or("an edit has no newText")?;
        let (a, b) = (offset_of(text, sl, sc), offset_of(text, el, ec));
        if b < a {
            return Err("an edit's range ends before it starts".into());
        }
        spans.push((a, b, new_text.to_string()));
    }
    // Stable: edits at one point keep the server's order.
    spans.sort_by_key(|s| (s.0, s.1));
    // The same replacement sent twice is one replacement (servers do this); it must not read as an overlap.
    spans.dedup_by(|b, a| a == b && a.1 > a.0);
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            return Err("the server sent edits that overlap".into());
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for (a, b, new_text) in spans {
        out.push_str(&text[at..a]);
        out.push_str(&new_text);
        at = b;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

fn plan(root: &str, edit: &Value) -> Result<Vec<Step>, String> {
    let checked = |uri: &str| -> Result<PathBuf, String> {
        let p = PathBuf::from(uri_to_path(uri));
        if inside_root(&p, root) {
            Ok(p)
        } else {
            Err(format!("the server tried to change {}, which is outside the workspace", p.display()))
        }
    };
    // The text each file will have, so several edits to one file stack instead of each reading the original.
    let mut texts: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    let mut steps: Vec<Step> = Vec::new();
    let edit_file = |texts: &mut BTreeMap<String, (PathBuf, String)>, uri: &str, edits: &[Value]| -> Result<(), String> {
        let path = checked(uri)?;
        let key = path_key(uri);
        let current = match texts.get(&key) {
            Some((_, t)) => t.clone(),
            None => std::fs::read_to_string(&path).map_err(|e| format!("can't read {}: {e}", path.display()))?,
        };
        let next = apply_text_edits(&current, edits)?;
        texts.insert(key, (path, next));
        Ok(())
    };

    if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            edit_file(&mut texts, uri, edits.as_array().map(|a| a.as_slice()).unwrap_or(&[]))?;
        }
    }
    if let Some(doc_changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in doc_changes {
            match change.get("kind").and_then(|k| k.as_str()) {
                Some("create") => {
                    let path = checked(change["uri"].as_str().ok_or("a create has no uri")?)?;
                    if path.exists() && !change.pointer("/options/overwrite").and_then(|v| v.as_bool()).unwrap_or(false) && !change.pointer("/options/ignoreIfExists").and_then(|v| v.as_bool()).unwrap_or(false) {
                        return Err(format!("{} already exists", path.display()));
                    }
                    if !path.exists() {
                        steps.push(Step::Create(path));
                    }
                }
                Some("rename") => {
                    let from = checked(change["oldUri"].as_str().ok_or("a rename has no oldUri")?)?;
                    let to = checked(change["newUri"].as_str().ok_or("a rename has no newUri")?)?;
                    if to.exists() && !change.pointer("/options/overwrite").and_then(|v| v.as_bool()).unwrap_or(false) {
                        return Err(format!("{} already exists", to.display()));
                    }
                    // Edits already planned for the old path follow the file.
                    if let Some((_, (_, text))) = texts.remove_entry(&path_key(&from.to_string_lossy())) {
                        steps.push(Step::Write(from.clone(), text));
                    }
                    steps.push(Step::Rename(from, to));
                }
                Some("delete") => steps.push(Step::Delete(checked(change["uri"].as_str().ok_or("a delete has no uri")?)?)),
                _ => {
                    let uri = change.pointer("/textDocument/uri").and_then(|u| u.as_str()).ok_or("a document change has no uri")?;
                    let edits: Vec<Value> = change.get("edits").and_then(|e| e.as_array()).cloned().unwrap_or_default();
                    // An annotated edit carries the same range and newText.
                    edit_file(&mut texts, uri, &edits)?;
                }
            }
        }
    }
    // Edits first, then the file operations that followed them, in the order given.
    let mut all: Vec<Step> = texts.into_values().map(|(p, t)| Step::Write(p, t)).collect();
    all.extend(steps);
    Ok(all)
}

/// Apply a `WorkspaceEdit` under `root`. Returns what changed, one line per file.
pub async fn apply_workspace_edit(root: &str, edit: &Value) -> Result<Vec<String>, String> {
    let steps = plan(root, edit)?;
    let mut done = Vec::new();
    for step in steps {
        match step {
            Step::Write(path, text) => {
                crate::files::guard::write_atomic(&path, text.as_bytes()).await.map_err(|e| format!("can't write {}: {e}", path.display()))?;
                done.push(format!("edited {}", path.display()));
            }
            Step::Create(path) => {
                if let Some(dir) = path.parent() {
                    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
                }
                tokio::fs::write(&path, b"").await.map_err(|e| format!("can't create {}: {e}", path.display()))?;
                done.push(format!("created {}", path.display()));
            }
            Step::Rename(from, to) => {
                if let Some(dir) = to.parent() {
                    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
                }
                tokio::fs::rename(&from, &to).await.map_err(|e| format!("can't rename {}: {e}", from.display()))?;
                done.push(format!("renamed {} to {}", from.display(), to.display()));
            }
            Step::Delete(path) => {
                if path.is_dir() {
                    tokio::fs::remove_dir_all(&path).await
                } else {
                    tokio::fs::remove_file(&path).await
                }
                .map_err(|e| format!("can't delete {}: {e}", path.display()))?;
                done.push(format!("deleted {}", path.display()));
            }
        }
    }
    Ok(done)
}

// ── Formatting options ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("viv-edits-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Value {
        json!({ "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } })
    }

    #[test]
    fn edits_apply_against_the_original_text_in_utf16_columns() {
        let text = "let a😀 = 1;\nlet b = 2;\n";
        // "a😀" is a, then two UTF-16 units: the `=` starts at column 8.
        let edits = vec![json!({ "range": range(0, 4, 0, 7), "newText": "x" }), json!({ "range": range(1, 4, 1, 5), "newText": "bee" })];
        assert_eq!(apply_text_edits(text, &edits).unwrap(), "let x = 1;\nlet bee = 2;\n");
        // Given out of order, and an insert at the very end.
        let edits = vec![json!({ "range": range(1, 11, 1, 11), "newText": " // end" }), json!({ "range": range(0, 0, 0, 3), "newText": "const" })];
        assert_eq!(apply_text_edits(text, &edits).unwrap(), "const a😀 = 1;\nlet b = 2; // end\n");
    }

    #[test]
    fn overlapping_edits_are_refused() {
        let edits = vec![json!({ "range": range(0, 0, 0, 4), "newText": "a" }), json!({ "range": range(0, 2, 0, 6), "newText": "b" })];
        assert!(apply_text_edits("abcdefgh", &edits).unwrap_err().contains("overlap"));
    }

    #[test]
    fn crlf_lines_end_before_the_carriage_return() {
        let edits = vec![json!({ "range": range(0, 3, 0, 99), "newText": "X" })];
        assert_eq!(apply_text_edits("abc\r\ndef", &edits).unwrap(), "abcX\r\ndef");
    }

    #[tokio::test]
    async fn a_workspace_edit_changes_creates_renames_and_deletes() {
        let root = tmp();
        let (a, gone, moved) = (root.join("a.txt"), root.join("gone.txt"), root.join("old.txt"));
        std::fs::write(&a, "hello world").unwrap();
        std::fs::write(&gone, "x").unwrap();
        std::fs::write(&moved, "m").unwrap();
        let edit = json!({ "documentChanges": [
            { "textDocument": { "uri": super::super::uri::path_to_uri(&a.to_string_lossy()), "version": null }, "edits": [{ "range": range(0, 0, 0, 5), "newText": "bye" }] },
            { "textDocument": { "uri": super::super::uri::path_to_uri(&a.to_string_lossy()), "version": null }, "edits": [{ "range": range(0, 4, 0, 9), "newText": "all" }] },
            { "kind": "create", "uri": super::super::uri::path_to_uri(&root.join("sub/new.txt").to_string_lossy()) },
            { "kind": "delete", "uri": super::super::uri::path_to_uri(&gone.to_string_lossy()) },
            { "kind": "rename", "oldUri": super::super::uri::path_to_uri(&moved.to_string_lossy()), "newUri": super::super::uri::path_to_uri(&root.join("new.txt").to_string_lossy()) },
        ] });
        let done = apply_workspace_edit(&root.to_string_lossy(), &edit).await.unwrap();
        assert_eq!(done.len(), 4, "{done:?}");
        // Two document changes to one file stack; the second's columns are about the text the first left.
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "bye all");
        assert!(root.join("sub/new.txt").exists() && !gone.exists() && !moved.exists() && root.join("new.txt").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_path_outside_the_workspace_refuses_the_whole_edit() {
        let root = tmp();
        let inside = root.join("in.txt");
        std::fs::write(&inside, "keep").unwrap();
        let outside = tmp().join("out.txt");
        std::fs::write(&outside, "keep").unwrap();
        let sneaky = format!("{}/../{}", root.display(), outside.file_name().unwrap().to_string_lossy());
        for target in [outside.to_string_lossy().to_string(), sneaky] {
            let edit = json!({ "changes": {
                super::super::uri::path_to_uri(&inside.to_string_lossy()): [{ "range": range(0, 0, 0, 4), "newText": "changed" }],
                super::super::uri::path_to_uri(&target): [{ "range": range(0, 0, 0, 4), "newText": "changed" }],
            } });
            let err = apply_workspace_edit(&root.to_string_lossy(), &edit).await.unwrap_err();
            assert!(err.contains("outside the workspace"), "{err}");
            assert_eq!(std::fs::read_to_string(&inside).unwrap(), "keep", "nothing is written when any path is refused");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_snippet_edit_is_refused_and_a_repeated_edit_is_one() {
        let snippet = vec![json!({ "range": range(0, 0, 0, 0), "newText": "fn ${1:name}() {}", "insertTextFormat": 2 })];
        assert!(apply_text_edits("", &snippet).unwrap_err().contains("snippet"));
        let twice = vec![json!({ "range": range(0, 0, 0, 3), "newText": "X" }), json!({ "range": range(0, 0, 0, 3), "newText": "X" })];
        assert_eq!(apply_text_edits("abcdef", &twice).unwrap(), "Xdef");
    }

}
