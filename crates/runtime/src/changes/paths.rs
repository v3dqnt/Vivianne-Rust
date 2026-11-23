//! Path classification and display forms for changed files.

use std::path::{Component, Path, PathBuf};

/// The slash-separated form of a path.
#[must_use]
pub fn to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Whether `path` is `root` or lies under it.
#[must_use]
pub fn is_inside(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

/// `path` without Windows' `\\?\` verbatim prefix, so canonical paths compare with the ones git prints.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// The symlink-resolved path. One that does not exist yet is resolved through its nearest existing ancestor, so a file
/// created through a folder link has the same spelling before and after it exists. A path with no existing ancestor
/// keeps its spelling.
#[must_use]
pub fn canonical_path(path: &Path) -> PathBuf {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut head = path.to_path_buf();
    loop {
        if let Ok(real) = std::fs::canonicalize(&head) {
            let mut out = plain(real);
            for part in missing.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let (Some(parent), Some(name)) = (head.parent().map(Path::to_path_buf), head.file_name())
        else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        head = parent;
    }
}

/// Where scratch work lives: the host `/tmp` and the platform temporary folder, each also in its resolved form.
#[must_use]
pub fn temporary_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in [PathBuf::from("/tmp"), std::env::temp_dir()] {
        let real = canonical_path(&root);
        for candidate in [root, real] {
            if !roots.contains(&candidate) {
                roots.push(candidate);
            }
        }
    }
    roots
}

/// Whether a file is under a temporary root, where the model keeps scratch work.
#[must_use]
pub fn is_temporary_path(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| is_inside(root, path))
}

/// The home folder, as the platform names it; empty when unknown.
#[must_use]
pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// `path` relative to `base`, using `..` where it must climb. Both are absolute and canonical.
fn relative(base: &Path, path: &Path) -> PathBuf {
    let base: Vec<Component<'_>> = base.components().collect();
    let target: Vec<Component<'_>> = path.components().collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..base.len() {
        out.push("..");
    }
    for part in &target[common..] {
        out.push(part.as_os_str());
    }
    out
}

/// Sort key and label of a changed file; see `ChangedFile::display`.
#[must_use]
pub fn display_path_of(absolute: &Path, cwd: &Path, root: &Path, home: &Path) -> String {
    if is_inside(cwd, absolute) || is_inside(root, absolute) {
        return to_posix(&relative(cwd, absolute));
    }
    if !home.as_os_str().is_empty() && is_inside(home, absolute) {
        return format!("~/{}", to_posix(&relative(home, absolute)));
    }
    to_posix(absolute)
}

/// The durable `path`: relative inside the working folder, absolute elsewhere.
#[must_use]
pub fn durable_path_of(absolute: &Path, cwd: &Path) -> String {
    if is_inside(cwd, absolute) {
        to_posix(&relative(cwd, absolute))
    } else {
        absolute.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_durable_forms_follow_where_the_file_lives() {
        let (cwd, root, home) = (Path::new("/r/app"), Path::new("/r"), Path::new("/h"));
        assert_eq!(
            display_path_of(Path::new("/r/app/src/a.rs"), cwd, root, home),
            "src/a.rs"
        );
        assert_eq!(
            display_path_of(Path::new("/r/lib/b.rs"), cwd, root, home),
            "../lib/b.rs"
        );
        assert_eq!(
            display_path_of(Path::new("/h/notes/c.md"), cwd, root, home),
            "~/notes/c.md"
        );
        assert_eq!(
            display_path_of(Path::new("/x/d.txt"), cwd, root, home),
            "/x/d.txt"
        );
        assert_eq!(
            durable_path_of(Path::new("/r/app/src/a.rs"), cwd),
            "src/a.rs"
        );
        assert_eq!(
            durable_path_of(Path::new("/r/lib/b.rs"), cwd),
            "/r/lib/b.rs"
        );
    }

    #[test]
    fn parent_and_absolute_paths_sort_before_the_folders_own_files() {
        let mut shown = vec!["src/a.rs", "../lib/b.rs", "~/n.md", "/x/d.txt"];
        shown.sort_unstable();
        assert_eq!(shown, ["../lib/b.rs", "/x/d.txt", "src/a.rs", "~/n.md"]);
    }

    #[test]
    fn a_path_that_does_not_exist_resolves_through_its_nearest_real_ancestor() {
        let dir = std::env::temp_dir().join(format!("viv-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let real = canonical_path(&dir);
        assert_eq!(
            canonical_path(&dir.join("a/b/new.txt")),
            real.join("a/b/new.txt")
        );
        assert!(is_temporary_path(&real.join("x"), &temporary_roots()));
        assert!(!is_temporary_path(
            Path::new("/definitely/not/tmp"),
            &temporary_roots()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
