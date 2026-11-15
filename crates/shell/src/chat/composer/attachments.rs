//! Files attached to a message, and how they travel with it. No UI types, so it is tested.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Largest text file pasted into the message. Bigger ones are only named.
const INLINE_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
}

impl AttachedFile {
    /// `None` if `path` is not a readable file.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path)
            .ok()
            .filter(std::fs::Metadata::is_file)?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some(Self {
            path: path.to_path_buf(),
            name,
            size: meta.len(),
        })
    }
}

/// "812 B", "12 KB", "3.4 MB".
#[must_use]
pub fn size_label(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        0..KB => format!("{bytes} B"),
        KB..MB => format!("{} KB", bytes / KB),
        _ => format!("{}.{} MB", bytes / MB, (bytes % MB) * 10 / MB),
    }
}

/// What the model receives: the typed text, then each attached file. Small text
/// files are included in full; anything else is named by its path so the agent
/// can open it with its own tools.
#[must_use]
pub fn compose_message(text: &str, files: &[AttachedFile]) -> String {
    let mut message = text.trim().to_owned();
    for file in files {
        message.push_str("\n\n");
        let _ = match inline_text(file) {
            Some(body) => write!(message, "Attached file `{}`:\n```\n{body}\n```", file.name),
            None => write!(
                message,
                "Attached file `{}` ({}), not included here. Path: {}",
                file.name,
                size_label(file.size),
                file.path.display()
            ),
        };
    }
    message
}

fn inline_text(file: &AttachedFile) -> Option<String> {
    if file.size > INLINE_MAX_BYTES {
        return None;
    }
    String::from_utf8(std::fs::read(&file.path).ok()?).ok()
}

/// What the transcript shows for the user's message: the text and the file names.
#[must_use]
pub fn display_message(text: &str, files: &[AttachedFile]) -> String {
    let text = text.trim();
    if files.is_empty() {
        return text.to_owned();
    }
    let names = files
        .iter()
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if text.is_empty() {
        format!("Attached: {names}")
    } else {
        format!("{text}\n\nAttached: {names}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-attach-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size_label(812), "812 B");
        assert_eq!(size_label(12 * 1024), "12 KB");
        assert_eq!(size_label(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
    }

    #[test]
    fn a_folder_is_not_an_attachment() {
        let dir = scratch("folder");
        assert!(AttachedFile::from_path(&dir).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn small_text_files_are_inlined_and_binary_ones_are_named() {
        let dir = scratch("compose");
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("b.bin"), [0xff, 0xfe, 0x00]).unwrap();
        let files: Vec<_> = ["a.txt", "b.bin"]
            .iter()
            .map(|n| AttachedFile::from_path(&dir.join(n)).unwrap())
            .collect();
        let message = compose_message("look at these", &files);
        assert!(message.starts_with("look at these"));
        assert!(message.contains("Attached file `a.txt`:\n```\nhello\n```"));
        assert!(message.contains("Attached file `b.bin` (3 B), not included here."));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn display_lists_names_and_handles_no_text() {
        let file = AttachedFile {
            path: "x/a.rs".into(),
            name: "a.rs".into(),
            size: 1,
        };
        assert_eq!(
            display_message("fix it", std::slice::from_ref(&file)),
            "fix it\n\nAttached: a.rs"
        );
        assert_eq!(display_message("  ", &[file]), "Attached: a.rs");
        assert_eq!(display_message(" hi ", &[]), "hi");
    }
}
