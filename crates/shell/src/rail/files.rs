//! The file tree shown in the rail. Pure filesystem logic, no UI types.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Folders that are never worth browsing.
const SKIPPED_DIRS: [&str; 3] = [".git", "target", "node_modules"];
/// Largest file the preview will read.
pub const PREVIEW_MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

/// The rows to show: folders first, then files, each sorted by name; the
/// children of expanded folders follow their folder, one level deeper.
#[must_use]
pub fn visible_rows(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<Row> {
    let mut rows = Vec::new();
    push_children(root, 0, expanded, &mut rows);
    rows
}

fn push_children(dir: &Path, depth: usize, expanded: &HashSet<PathBuf>, rows: &mut Vec<Row>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(String, PathBuf, bool)> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().ok()?.is_dir();
            if is_dir && SKIPPED_DIRS.contains(&name.as_str()) {
                return None;
            }
            Some((name, entry.path(), is_dir))
        })
        .collect();
    entries.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    for (name, path, is_dir) in entries {
        let open = is_dir && expanded.contains(&path);
        rows.push(Row {
            path: path.clone(),
            name,
            depth,
            is_dir,
            expanded: open,
        });
        if open {
            push_children(&path, depth + 1, expanded, rows);
        }
    }
}

/// What the preview can show for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Markdown(String),
    Image,
    Unavailable(String),
}

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Reads a file for the preview, or says why it cannot be shown.
#[must_use]
pub fn classify(path: &Path) -> Content {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return Content::Image;
    }
    match read_preview(path) {
        Ok(text) if extension == "md" || extension == "markdown" => Content::Markdown(text),
        Ok(text) => Content::Text(text),
        Err(why) => Content::Unavailable(why),
    }
}

/// Text of a file for the preview, or why it cannot be shown.
pub fn read_preview(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > PREVIEW_MAX_BYTES {
        return Err(format!("Too large to preview ({} KB)", size / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| "Not a text file".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("src").join("lib.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("A.txt"), "a").unwrap();
        dir
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn folders_come_first_then_files_ignoring_case_and_skipped_dirs() {
        let root = scratch("order");
        let rows = visible_rows(&root, &HashSet::new());
        assert_eq!(names(&rows), ["src", "A.txt", "b.txt"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn expanding_a_folder_shows_its_children_one_level_deeper() {
        let root = scratch("expand");
        let open: HashSet<PathBuf> = [root.join("src")].into();
        let rows = visible_rows(&root, &open);
        assert_eq!(names(&rows), ["src", "lib.rs", "A.txt", "b.txt"]);
        assert_eq!((rows[0].depth, rows[0].expanded), (0, true));
        assert_eq!(rows[1].depth, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_are_classified_by_what_they_hold() {
        let root = scratch("classify");
        std::fs::write(root.join("r.md"), "# hi").unwrap();
        std::fs::write(root.join("p.PNG"), [1, 2]).unwrap();
        assert_eq!(classify(&root.join("b.txt")), Content::Text("b".into()));
        assert_eq!(
            classify(&root.join("r.md")),
            Content::Markdown("# hi".into())
        );
        assert_eq!(classify(&root.join("p.PNG")), Content::Image);
        assert!(matches!(
            classify(&root.join("missing")),
            Content::Unavailable(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_reads_text_and_refuses_binary() {
        let root = scratch("preview");
        assert_eq!(read_preview(&root.join("b.txt")).unwrap(), "b");
        std::fs::write(root.join("bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            read_preview(&root.join("bin")).unwrap_err(),
            "Not a text file"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
