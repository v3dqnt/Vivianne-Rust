//! A conversation as a Markdown transcript, and as a ZIP with the log, the transcript, the conversations branched from
//! it and the files attached along the way. Port of the file side of the original `session-log-export`; the Web
//! controls it had are not here, only what the app and `viv thread export` call.
//!
//! Secrets are taken out of the log and the transcript unless the person turns that off. The bytes of an attachment are
//! not scanned: a file is put in the archive as it was attached.

use std::collections::BTreeSet;

use serde::Serialize;
use vivianne_core::attachment::AttachmentRef;
use vivianne_core::{Message, Role};

use crate::archive::{ArchiveError, Zip};
use crate::export::{self, Options, redact};
use crate::query::{Query, QueryError, trace::LineageNode};
use crate::store::SavedThread;

/// Branches included in an archive; the manifest says how many were left out.
pub const MAX_BRANCHES: usize = 50;
/// Longest a tool result or reply is in the transcript before it is cut.
const MAX_BLOCK: usize = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("{0}")]
    Query(#[from] QueryError),
    #[error("{0}")]
    Archive(#[from] ArchiveError),
    #[error("{0}")]
    Write(String),
}

fn shown(text: &str, options: Options) -> String {
    let text = if options.redact {
        redact(text)
    } else {
        text.to_owned()
    };
    if text.chars().count() > MAX_BLOCK {
        let kept: String = text.chars().take(MAX_BLOCK).collect();
        return format!(
            "{kept}\n[… {} more characters left out]",
            text.chars().count() - MAX_BLOCK
        );
    }
    text
}

/// A code fence longer than any run of backticks in `text`, so the text cannot close it early.
fn fence(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat((longest + 1).max(3))
}

fn block(out: &mut String, label: &str, text: &str) {
    let f = fence(text);
    out.push_str(&format!("{f}{label}\n{text}\n{f}\n\n"));
}

/// The transcript: one heading per message, replies and tool calls in order. App-added context and condensed history
/// are named but not printed.
#[must_use]
pub fn markdown(thread: &SavedThread, options: Options) -> String {
    let mut out = format!(
        "# {}\n\n",
        shown(thread.title.trim(), options).replace('\n', " ")
    );
    out.push_str(&format!(
        "- Thread: `{}`\n- Model: {} ({})\n- Messages: {}\n\n---\n\n",
        thread.id,
        thread.model,
        thread.provider,
        thread.history.len()
    ));
    for (seq, message) in thread.history.iter().enumerate() {
        out.push_str(&message_markdown(seq, message, options));
    }
    out
}

fn message_markdown(seq: usize, message: &Message, options: Options) -> String {
    use crate::query::extract::kind_of;
    use crate::query::filters::Kind;
    let mut out = String::new();
    match kind_of(message) {
        Kind::Context => out.push_str(&format!(
            "## {seq} · Context\n\n_added by the app for the model_\n\n"
        )),
        Kind::Checkpoint => out.push_str(&format!("## {seq} · Earlier conversation condensed\n\n")),
        Kind::User => {
            out.push_str(&format!("## {seq} · You\n\n"));
            if !message.text.trim().is_empty() {
                out.push_str(&format!("{}\n\n", shown(message.text.trim(), options)));
            }
        }
        Kind::Assistant => {
            out.push_str(&format!("## {seq} · Assistant\n\n"));
            if !message.text.trim().is_empty() {
                out.push_str(&format!("{}\n\n", shown(message.text.trim(), options)));
            }
            for call in &message.tool_calls {
                let args = serde_json::to_string_pretty(&call.args).unwrap_or_default();
                block(
                    &mut out,
                    &format!("json\n// tool: {}", call.name),
                    &shown(&args, options),
                );
            }
        }
        Kind::ToolResult => {
            out.push_str(&format!("## {seq} · Tool result\n\n"));
            block(&mut out, "", &shown(message.text.trim(), options));
        }
    }
    if !message.attachments.is_empty() {
        let names: Vec<String> = message
            .attachments
            .iter()
            .map(|a| format!("`{}`", a.display_name()))
            .collect();
        out.push_str(&format!("Attached: {}\n\n", names.join(", ")));
    }
    if message.role == Role::System {
        out.push_str("_system_\n\n");
    }
    out
}

/// Reads an attachment's bytes from the attachment store under `~/.viv`; `None` when it is gone or unreadable.
pub fn standard_reader() -> impl Fn(&AttachmentRef) -> Option<Vec<u8>> {
    let store = vivianne_ai::paths::viv_home().ok().and_then(|home| {
        vivianne_attachments::Store::new(&home, vivianne_attachments::Config::default()).ok()
    });
    move |attachment| {
        let store = store.as_ref()?;
        let path = match attachment {
            AttachmentRef::Image(i) => store.image_host_path(i).ok()?,
            AttachmentRef::File(f) => store.file_host_path(f).ok()?,
        };
        std::fs::read(path).ok()
    }
}

/// What went into an archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Manifest {
    pub format: &'static str,
    pub version: u32,
    pub thread: String,
    pub redacted: bool,
    pub branches: Vec<String>,
    /// Branches past [`MAX_BRANCHES`].
    pub branches_left_out: usize,
    pub attachments: Vec<String>,
    /// Attachments whose bytes could not be read, by name.
    pub attachments_missing: Vec<String>,
}

fn flatten(nodes: &[LineageNode], out: &mut Vec<String>) {
    for node in nodes {
        out.push(node.thread.id.clone());
        flatten(&node.descendants, out);
    }
}

fn file_name(attachment: &AttachmentRef) -> String {
    let (digest, name) = match attachment {
        AttachmentRef::Image(i) => (i.attachment_id.digest(), attachment.display_name()),
        AttachmentRef::File(f) => (f.attachment_id.digest(), f.name.as_str()),
    };
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachments/{}-{}",
        &digest[..digest.len().min(8)],
        clean.trim_start_matches('.')
    )
}

/// The archive for thread `id`: `session.jsonl`, `session.md`, `branches/<id>/session.jsonl` and `.md` for each
/// conversation branched from it, `attachments/` and `manifest.json`. `read` supplies an attachment's bytes.
pub fn bundle(
    query: &Query,
    id: &str,
    options: Options,
    read: &dyn Fn(&AttachmentRef) -> Option<Vec<u8>>,
) -> Result<(Vec<u8>, Manifest), BundleError> {
    let thread = query.read_thread(id)?;
    let mut ids = Vec::new();
    flatten(&query.trace_thread(id)?.descendants, &mut ids);
    let left_out = ids.len().saturating_sub(MAX_BRANCHES);
    ids.truncate(MAX_BRANCHES);
    let mut zip = Zip::new();
    let mut manifest = Manifest {
        format: "vivianne-session-archive",
        version: 1,
        thread: id.to_owned(),
        redacted: options.redact,
        branches: Vec::new(),
        branches_left_out: left_out,
        attachments: Vec::new(),
        attachments_missing: Vec::new(),
    };
    let mut written = BTreeSet::new();
    let mut put = |zip: &mut Zip,
                   folder: &str,
                   t: &SavedThread,
                   manifest: &mut Manifest|
     -> Result<(), BundleError> {
        let mut jsonl = Vec::new();
        export::write_to(&mut jsonl, t, options).map_err(BundleError::Write)?;
        zip.add(&format!("{folder}session.jsonl"), &jsonl, t.updated_at)?;
        zip.add(
            &format!("{folder}session.md"),
            markdown(t, options).as_bytes(),
            t.updated_at,
        )?;
        for attachment in t.history.iter().flat_map(|m| &m.attachments) {
            let name = file_name(attachment);
            if !written.insert(name.clone()) {
                continue;
            }
            match read(attachment) {
                Some(bytes) => {
                    zip.add(&name, &bytes, t.updated_at)?;
                    manifest.attachments.push(name);
                }
                None => manifest
                    .attachments_missing
                    .push(attachment.display_name().to_owned()),
            }
        }
        Ok(())
    };
    put(&mut zip, "", &thread, &mut manifest)?;
    for branch in &ids {
        let t = query.read_thread(branch)?;
        put(&mut zip, &format!("branches/{branch}/"), &t, &mut manifest)?;
        manifest.branches.push(branch.clone());
    }
    let listing =
        serde_json::to_vec_pretty(&manifest).map_err(|e| BundleError::Write(e.to_string()))?;
    zip.add("manifest.json", &listing, thread.updated_at)?;
    Ok((zip.finish()?, manifest))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;

    use super::*;
    use crate::query::tests_support::{dir, thread};
    use crate::store::{fork_to, save_to};

    #[test]
    fn the_transcript_reads_in_order_cuts_what_is_huge_and_cannot_be_broken_out_of() {
        let mut t = thread("t", "/w", 1, &[]);
        t.title = "A **title**".into();
        let call = ToolCall {
            id: "c".into(),
            name: "shell".into(),
            args: json!({ "command": "ls" }),
        };
        t.history = vec![
            Message::new(Role::User, "run it with key sk-abcdefghijklmnopqrstuvwx"),
            Message::assistant("```rust\nfn main() {}\n```".to_owned(), vec![call]),
            Message::tool_result("c", "x".repeat(MAX_BLOCK + 50)),
            Message::new(Role::User, "<compacted_history>old</compacted_history>"),
        ];
        let text = markdown(&t, Options::default());
        let at = |needle: &str| text.find(needle).unwrap_or(usize::MAX);
        assert!(
            at("## 0 · You") < at("## 1 · Assistant")
                && at("## 1 · Assistant") < at("## 2 · Tool result")
        );
        assert!(text.contains("[REDACTED]") && !text.contains("sk-abcdef"));
        assert!(text.contains("// tool: shell"));
        assert!(text.contains("50 more characters left out"));
        assert!(text.contains("condensed") && !text.contains("old</compacted"));
        assert_eq!(
            fence("a ``` b ````` c"),
            "``````",
            "longer than any run inside"
        );
        let raw = markdown(
            &t,
            Options {
                redact: false,
                after: 0,
            },
        );
        assert!(raw.contains("sk-abcdefghijklmnopqrstuvwx"));
    }

    #[test]
    fn the_archive_holds_the_log_the_transcript_the_branches_and_a_manifest() {
        let dir = dir("bundle");
        save_to(
            &dir,
            &thread("root", "/w", 1_700_000_000, &["one", "a", "two", "b"]),
        )
        .expect("saved");
        let branch = fork_to(&dir, "root", 2).expect("forked");
        let q = Query::new(&dir);
        let (bytes, manifest) = bundle(&q, "root", Options::default(), &|_| None).expect("bundled");
        assert_eq!(manifest.branches, [branch.id.clone()]);
        let text = String::from_utf8_lossy(&bytes);
        for name in [
            "session.jsonl",
            "session.md",
            "manifest.json",
            &format!("branches/{}/session.md", branch.id),
        ] {
            assert!(text.contains(name), "{name}");
        }
        assert!(matches!(
            bundle(&q, "nope", Options::default(), &|_| None),
            Err(BundleError::Query(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
