//! Finding paths for `@file` completion in a working directory. The index holds paths only: a chosen path is ordinary prompt
//! text, and what is in the file stays behind the `read` tool. Port of `file-reference-local`'s `search.ts`.
//!
//! Directory-scoped queries (`src/ma`) list the folder as it is. A bare query ranks a bounded walk of the whole tree. Only the
//! first query of a workspace waits for that walk: after a tool has run, the old index keeps answering while a new one builds
//! behind the caret. Folder symlinks are never followed.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::grammar::{Candidate, Kind};

pub const DEFAULT_MAX_RESULTS: usize = 20;
pub const DEFAULT_MAX_ENTRIES: usize = 50_000;
/// Directory names left out: version-control and dependency stores plus build output no ecosystem also uses for sources.
/// `lib` is deliberately absent: Ruby gems and many packages keep their sources there.
pub const DEFAULT_EXCLUDED: [&str; 15] = [
    ".git",
    "node_modules",
    "dist",
    "build",
    "out",
    "coverage",
    "target",
    ".next",
    ".nuxt",
    ".turbo",
    ".venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".gradle",
];

#[derive(Debug, Clone)]
pub struct Config {
    pub max_results: usize,
    pub max_entries: usize,
    pub excluded_directories: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_entries: DEFAULT_MAX_ENTRIES,
            excluded_directories: DEFAULT_EXCLUDED.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

impl Config {
    /// Every number is positive and every excluded name is a plain folder name.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_results == 0 || self.max_entries == 0 {
            return Err("file-reference: maxResults and maxEntries must be positive".into());
        }
        if self
            .excluded_directories
            .iter()
            .any(|n| n.is_empty() || n.contains(['/', '\\']))
        {
            return Err(
                "file-reference: excludedDirectories entries must be non-empty directory basenames"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Stops a search a caller no longer wants.
#[derive(Debug, Clone, Default)]
pub struct Stop(Arc<AtomicBool>);

impl Stop {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// How a search ended without an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    Stopped,
    Failed(String),
}

type Index = Arc<Vec<Candidate>>;

#[derive(Debug)]
struct Build {
    stop: Stop,
    result: Mutex<Option<Result<Index, String>>>,
    done: Condvar,
}

#[derive(Debug, Default)]
struct Inner {
    settled: Option<(Index, u64)>,
    building: Option<Arc<Build>>,
}

/// A reusable fuzzy index rooted at one working directory.
#[derive(Debug)]
pub struct WorkspaceSearch {
    root: PathBuf,
    config: Config,
    excluded: HashSet<String>,
    inner: Arc<Mutex<Inner>>,
    /// Counts tool results; a settled index below it is stale.
    invalidations: Arc<AtomicU64>,
    disposed: Arc<AtomicBool>,
}

impl WorkspaceSearch {
    pub fn new(root: &Path, config: Config) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            root: root.to_path_buf(),
            excluded: config.excluded_directories.iter().cloned().collect(),
            config,
            inner: Arc::default(),
            invalidations: Arc::default(),
            disposed: Arc::default(),
        })
    }

    /// Ranked candidates for the text after `@` or `@"`, at most `max_results`, in a fixed order.
    pub fn list(&self, raw_query: &str, stop: &Stop) -> Result<Vec<Candidate>, SearchError> {
        if stop.is_stopped() {
            return Err(SearchError::Stopped);
        }
        if self.disposed.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let query = raw_query.replace('\\', "/");
        if query.is_empty() || query.contains('/') {
            let (directory, fragment) = query
                .rfind('/')
                .map_or(("", ""), |i| (&query[..=i], &query[i + 1..]));
            return Ok(self.list_directory(directory, fragment));
        }
        let index = self.index(stop)?;
        let visible: Vec<Candidate> = index
            .iter()
            .filter(|c| visible_for_global_query(&c.path, &query))
            .cloned()
            .collect();
        Ok(rank(&visible, &query, self.config.max_results))
    }

    /// Marks the index stale. It keeps answering; the next bare query starts a replacement in the background.
    pub fn invalidate(&self) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }

    /// Stops any walk and makes later queries return nothing.
    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(build) = inner.building.take() {
            build.stop.stop();
        }
        inner.settled = None;
    }

    fn index(&self, stop: &Stop) -> Result<Index, SearchError> {
        let settled = self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .settled
            .clone();
        match settled {
            None => Self::wait(&self.ensure_build(), stop),
            Some((entries, started_at)) => {
                if started_at < self.invalidations.load(Ordering::SeqCst) {
                    // A failed refresh is not this caller's error: the old entries still answer.
                    let _ = self.ensure_build();
                }
                Ok(entries)
            }
        }
    }

    fn ensure_build(&self) -> Arc<Build> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(build) = &inner.building {
            return build.clone();
        }
        let build = Arc::new(Build {
            stop: Stop::new(),
            result: Mutex::new(None),
            done: Condvar::new(),
        });
        inner.building = Some(build.clone());
        drop(inner);
        let (root, max, excluded) = (
            self.root.clone(),
            self.config.max_entries,
            self.excluded.clone(),
        );
        let started_at = self.invalidations.load(Ordering::SeqCst);
        let (shared, worker, disposed) = (self.inner.clone(), build.clone(), self.disposed.clone());
        std::thread::spawn(move || {
            let outcome = scan(&root, max, &excluded, &worker.stop).map(Arc::new);
            {
                let mut inner = shared.lock().unwrap_or_else(PoisonError::into_inner);
                if inner
                    .building
                    .as_ref()
                    .is_some_and(|b| Arc::ptr_eq(b, &worker))
                {
                    inner.building = None;
                    if let (Ok(entries), false) = (&outcome, disposed.load(Ordering::SeqCst)) {
                        inner.settled = Some((entries.clone(), started_at));
                    }
                }
            }
            *worker.result.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            worker.done.notify_all();
        });
        build
    }

    /// Waits for a walk, giving up when this caller's `stop` fires without stopping a walk a newer query shares.
    fn wait(build: &Arc<Build>, stop: &Stop) -> Result<Index, SearchError> {
        let mut result = build.result.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = result.as_ref() {
                return outcome.clone().map_err(SearchError::Failed);
            }
            if stop.is_stopped() {
                return Err(SearchError::Stopped);
            }
            result = build
                .done
                .wait_timeout(result, Duration::from_millis(20))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn list_directory(&self, display_directory: &str, fragment: &str) -> Vec<Candidate> {
        if display_directory
            .split('/')
            .any(|s| self.excluded.contains(s))
        {
            return Vec::new();
        }
        let Some(absolute) = resolve_display_directory(&self.root, display_directory) else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        for (name, kind) in read_dir_sorted(&absolute) {
            if name.starts_with('.') && !fragment.starts_with('.') {
                continue;
            }
            if kind == Kind::Directory && self.excluded.contains(&name) {
                continue;
            }
            candidates.push(Candidate {
                path: format!("{display_directory}{name}"),
                kind,
            });
        }
        rank(&candidates, fragment, self.config.max_results)
    }
}

/// The entries of a folder, by name. Links (to files or folders) are skipped, so a folder symlink is never followed.
fn read_dir_sorted(path: &Path) -> Vec<(String, Kind)> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Kind)> = entries
        .flatten()
        .filter_map(|e| {
            let kind = match e.file_type().ok()? {
                t if t.is_dir() => Kind::Directory,
                t if t.is_file() => Kind::File,
                _ => return None,
            };
            Some((e.file_name().to_string_lossy().into_owned(), kind))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn scan(
    root: &Path,
    max_entries: usize,
    excluded: &HashSet<String>,
    stop: &Stop,
) -> Result<Vec<Candidate>, String> {
    let mut indexed: Vec<Candidate> = Vec::new();
    let mut queue: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    let mut cursor = 0;
    while cursor < queue.len() && indexed.len() < max_entries {
        if stop.is_stopped() {
            return Err("file search stopped".into());
        }
        let (absolute, relative) = queue[cursor].clone();
        cursor += 1;
        // The root is not a subtree: an unreadable branch costs its own candidates, but an unreadable root means the walk
        // learned nothing, and publishing an empty index over good entries would leave nothing to retry from.
        if cursor == 1 {
            std::fs::read_dir(&absolute)
                .map_err(|e| format!("cannot read the workspace: {}", e.kind()))?;
        }
        for (name, kind) in read_dir_sorted(&absolute) {
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if kind == Kind::Directory {
                if excluded.contains(&name) {
                    continue;
                }
                indexed.push(Candidate {
                    path: path.clone(),
                    kind,
                });
                queue.push((absolute.join(&name), path));
            } else {
                indexed.push(Candidate { path, kind });
            }
            if indexed.len() >= max_entries {
                break;
            }
        }
    }
    Ok(indexed)
}

/// `root` joined with a display directory, only when every step is a real folder (no link) and it stays inside `root`.
fn resolve_display_directory(root: &Path, display: &str) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for segment in display.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if Path::new(segment)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return None;
        }
        current.push(segment);
        let meta = std::fs::symlink_metadata(&current).ok()?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return None;
        }
    }
    Some(current)
}

fn visible_for_global_query(path: &str, query: &str) -> bool {
    if query.starts_with('.') || query.contains("/.") {
        return true;
    }
    !path.split('/').any(|s| s.starts_with('.'))
}

fn kind_rank(kind: Kind) -> u8 {
    u8::from(kind != Kind::Directory)
}

/// The best `limit` candidates for `query`: directories first at equal score, then shorter, then by name.
#[must_use]
pub fn rank(candidates: &[Candidate], query: &str, limit: usize) -> Vec<Candidate> {
    let mut ranked: Vec<(i64, &Candidate)> = candidates
        .iter()
        .filter_map(|c| score(c, query).map(|s| (s, c)))
        .collect();
    ranked.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then(kind_rank(a.kind).cmp(&kind_rank(b.kind)))
            .then(if query.is_empty() {
                std::cmp::Ordering::Equal
            } else {
                a.path.chars().count().cmp(&b.path.chars().count())
            })
            .then(a.path.cmp(&b.path))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, c)| c.clone())
        .collect()
}

fn score(candidate: &Candidate, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let path = candidate.path.to_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path);
    let needle = query.to_lowercase();
    let bonus = if candidate.kind == Kind::Directory {
        25
    } else {
        0
    };
    if name == needle {
        return Some(1000 + bonus);
    }
    if name.starts_with(&needle) {
        return Some(900 + bonus);
    }
    if name.contains(&needle) {
        return Some(700 + bonus);
    }
    if path.contains(&needle) {
        return Some(500 + bonus);
    }
    subsequence_score(&path, &needle).map(|s| 300 + s + bonus)
}

fn subsequence_score(target: &str, query: &str) -> Option<i64> {
    let target: Vec<char> = target.chars().collect();
    let (mut index, mut gap) = (0usize, 0i64);
    for ch in query.chars() {
        let found = target[index..].iter().position(|c| *c == ch)? + index;
        gap += i64::try_from(found - index).unwrap_or(0);
        index = found + 1;
    }
    Some((100 - gap).max(0))
}
