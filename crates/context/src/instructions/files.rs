//! Finding and reading instruction files. Everything that touches the disk goes through `Fs`, so the rules around it (which
//! candidate wins, what a failed probe means) are tested with a made-up file system.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::config::Resolved;
use super::render::{
    AGENTS_GLOBAL_DIRECTORY, LoadedFile, Rendered, USER_GLOBAL_DIRECTORY, USER_GLOBAL_FILE,
    candidate_group, escape_project_display_path, render_set, scope_for_display_path,
};

/// What a probe of one path found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// A file, with a freshness token and its size when known.
    Present { version: String, size: Option<u64> },
    /// Confirmed not there, or not a file.
    Absent,
    /// The check itself failed: nothing is known.
    Unavailable,
}

/// The disk, as the loader sees it.
pub trait Fs {
    /// A file at `path`; a link to a file counts, a link to a folder does not.
    fn probe(&self, path: &Path) -> Probe;
    /// Whether `path` exists, for a root marker (`.git` may be a folder or a file). An error is not "absent".
    fn marker_exists(&self, path: &Path) -> std::io::Result<bool>;
    /// The text of `path` as UTF-8, or `None` when it is over `max_bytes` or cannot be read.
    fn read_text(&self, path: &Path, max_bytes: u64) -> Option<String>;
}

/// The real disk.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFs;

fn is_missing(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

impl Fs for LocalFs {
    fn probe(&self, path: &Path) -> Probe {
        // `metadata` follows a final symlink, so a link to a file loads and a broken link is absent.
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => {
                let nanos = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                Probe::Present {
                    version: format!("{nanos}:{}", m.len()),
                    size: Some(m.len()),
                }
            }
            Ok(_) => Probe::Absent,
            Err(e) if is_missing(e.kind()) => Probe::Absent,
            Err(_) => Probe::Unavailable,
        }
    }

    fn marker_exists(&self, path: &Path) -> std::io::Result<bool> {
        match std::fs::metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if is_missing(e.kind()) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn read_text(&self, path: &Path, max_bytes: u64) -> Option<String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(max_bytes + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= max_bytes).then(|| String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The SHA-256 of exact content, in hex.
#[must_use]
pub fn content_digest(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The digest of trimmed content: two files that differ only by the whitespace around them are one file.
#[must_use]
pub fn trimmed_digest(content: &str) -> String {
    content_digest(content.trim())
}

/// One candidate found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub absolute: PathBuf,
    pub display: String,
    pub version: Option<String>,
    pub size: Option<u64>,
}

fn normalize(path: &Path) -> PathBuf {
    path.components().fold(PathBuf::new(), |mut out, c| {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
        out
    })
}

/// Walks up to the first directory with a root marker; the working directory itself when none has. A marker probe that
/// fails stops the walk with its error: another project's files are never substituted.
pub fn find_project_root(cwd: &Path, markers: &[String], fs: &dyn Fs) -> std::io::Result<PathBuf> {
    let start = normalize(cwd);
    let mut current = start.clone();
    loop {
        for marker in markers {
            if fs.marker_exists(&current.join(marker))? {
                return Ok(current);
            }
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return Ok(start),
        }
    }
}

/// The folders from `root` down to `cwd`, broadest first.
#[must_use]
pub fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let (root, cwd) = (normalize(root), normalize(cwd));
    let mut chain = Vec::new();
    let mut current = cwd;
    while current != root {
        chain.push(current.clone());
        match current.parent() {
            Some(p) if p != current => current = p.to_path_buf(),
            _ => break,
        }
    }
    chain.push(root);
    chain.reverse();
    chain
}

/// The folders below `root` that a path crosses, shallowest first, up to the folder of the path itself.
#[must_use]
pub fn descendant_dirs_between(root: &Path, touched: &str) -> Vec<PathBuf> {
    let root = normalize(root);
    let path = Path::new(touched);
    let target = if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&root.join(path))
    };
    let Some(dir) = target.parent() else {
        return Vec::new();
    };
    if dir == root || !dir.starts_with(&root) {
        return Vec::new();
    }
    ancestor_chain(&root, dir).into_iter().skip(1).collect()
}

/// A project path as the model sees it: relative to the root, with `/`.
#[must_use]
pub fn relative_display(root: &Path, path: &Path) -> String {
    let relative = normalize(path)
        .strip_prefix(normalize(root))
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf);
    escape_project_display_path(
        &relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn existing_in(dir: &Path, root: &Path, names: &[String], fs: &dyn Fs) -> Vec<Discovered> {
    names
        .iter()
        .filter_map(|name| {
            let path = dir.join(name);
            match fs.probe(&path) {
                Probe::Present { version, size } => Some(Discovered {
                    display: relative_display(root, &path),
                    absolute: path,
                    version: Some(version),
                    size,
                }),
                // A candidate that is missing or cannot be checked is skipped; the others still load.
                Probe::Absent | Probe::Unavailable => None,
            }
        })
        .collect()
}

/// The two user-global roots, in the order the model gives them precedence.
fn user_global_roots(resolved: &Resolved) -> [(&'static str, &Path, &str); 2] {
    [
        (
            USER_GLOBAL_DIRECTORY,
            resolved.viv_home.as_path(),
            resolved.viv_home_display.as_str(),
        ),
        (
            AGENTS_GLOBAL_DIRECTORY,
            resolved.agents_home.as_path(),
            resolved.agents_home_display.as_str(),
        ),
    ]
}

/// Every instruction file that applies, in the order the model reads them: the user-global files, then each project
/// directory from the root down to the working directory (base files, then local overlays).
pub fn discover(
    resolved: &Resolved,
    cwd: &Path,
    project_root: &Path,
    fs: &dyn Fs,
) -> Vec<Discovered> {
    let mut files: Vec<Discovered> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut add = |file: Discovered| {
        if seen.insert(file.absolute.clone()) {
            files.push(file);
        }
    };
    for (_, home, display) in user_global_roots(resolved) {
        let path = home.join(USER_GLOBAL_FILE);
        if let Probe::Present { version, size } = fs.probe(&path) {
            add(Discovered {
                absolute: path,
                display: format!("{display}/{USER_GLOBAL_FILE}"),
                version: Some(version),
                size,
            });
        }
    }
    for dir in ancestor_chain(project_root, cwd) {
        for names in [
            &resolved.config.instruction_file_candidates,
            &resolved.config.local_instruction_file_candidates,
        ] {
            for file in existing_in(&dir, project_root, names, fs) {
                add(file);
            }
        }
    }
    files
}

/// Drops later candidates whose trimmed content repeats an earlier one in the same candidate group. Different groups never
/// merge, even when identical. The earliest candidate keeps its exact bytes.
#[must_use]
pub fn dedup_by_group(files: Vec<LoadedFile>) -> Vec<LoadedFile> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    files
        .into_iter()
        .filter(|f| {
            seen.insert((
                candidate_group(&scope_for_display_path(&f.display)),
                trimmed_digest(&f.content),
            ))
        })
        .collect()
}

/// A baseline with the files it was made from.
#[derive(Debug, Clone)]
pub struct BaselineSet {
    pub rendered: Rendered,
    /// Files read successfully, before de-duplication and the budget.
    pub observed: Vec<LoadedFile>,
    /// Files dropped as repeats of an earlier candidate in their group.
    pub deduped: Vec<LoadedFile>,
    /// Files the rendered text stands for.
    pub included: Vec<LoadedFile>,
}

/// Finds, reads and renders the baseline. `None` when loading is off or there is nothing to load (unless a previous
/// baseline must be replaced by an empty one).
pub fn load_baseline(
    resolved: &Resolved,
    cwd: &Path,
    project_root: &Path,
    replace_previous: bool,
    fs: &dyn Fs,
) -> Option<BaselineSet> {
    if !resolved.enabled() {
        return None;
    }
    let loaded: Vec<LoadedFile> = discover(resolved, cwd, project_root, fs)
        .into_iter()
        .filter_map(|d| {
            let content = fs.read_text(&d.absolute, resolved.config.max_source_bytes)?;
            Some(LoadedFile {
                absolute: d.absolute.display().to_string(),
                display: d.display,
                content,
                version: d.version,
            })
        })
        .collect();
    let kept = dedup_by_group(loaded.clone());
    if kept.is_empty() && !replace_previous {
        return None;
    }
    let (rendered, included) = render_set(&kept, resolved.config.max_bytes, replace_previous);
    let deduped = loaded
        .iter()
        .filter(|f| !kept.iter().any(|k| k.absolute == f.absolute))
        .cloned()
        .collect();
    Some(BaselineSet {
        rendered,
        observed: loaded,
        deduped,
        included,
    })
}
