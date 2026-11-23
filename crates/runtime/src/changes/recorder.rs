//! One conversation's recorder: a snapshot at the start of each run, a whole-file capture before each file-tool edit, a
//! snapshot and a comparison at the end, and the summaries kept until the recorder is dropped. Everything runs under one
//! lock, so a snapshot or a capture never races another.
//!
//! A working folder outside any repository, or a machine without git, gets no snapshot; its summary lists the files the
//! file tools changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

use super::capture::{Capture, capture_file, same_capture};
use super::compare::compare_text;
use super::git::{self, Runner, Workspace};
use super::paths::{
    canonical_path, display_path_of, durable_path_of, home_dir, is_inside, is_temporary_path,
    temporary_roots, to_posix,
};
use super::types::{ChangedFile, ChangesSummary, FileDiff, Limits, SnapshotIds};

/// Canonical paths every comparison and display uses, resolved once.
#[derive(Debug, Clone)]
struct Paths {
    cwd: PathBuf,
    home: PathBuf,
    temporary: Vec<PathBuf>,
}

/// The repository enclosing the working folder.
#[derive(Debug, Clone)]
struct Repository {
    workspace: Workspace,
}

/// The start-of-run snapshot of a located repository.
#[derive(Debug, Clone)]
struct Baseline {
    repository: Repository,
    tree: String,
}

/// Where one readable side of a listed file lives.
#[derive(Debug, Clone)]
enum Source {
    /// A path in a snapshot tree; its size and text are read from git when asked for.
    Snapshot {
        repository: Repository,
        tree: String,
        path: String,
    },
    Absent,
    File(PathBuf),
}

/// What a listed file's comparison is served from: a refusal decided when the run was recorded, or the two sides.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
enum Sources {
    Refused(Refusal),
    Sides { before: Source, after: Source },
}

#[derive(Debug, Clone, Copy)]
enum Refusal {
    Binary,
    Oversized,
}

struct Record {
    summary: ChangesSummary,
    sources: Vec<Sources>,
}

enum BaselineState {
    /// No repository or no git: the run summarizes captures only.
    None,
    Taken(Box<Baseline>),
    /// The repository exists but its snapshot failed: the run records nothing.
    Failed,
}

/// What one run accumulates.
struct RunState {
    turn: usize,
    baseline: BaselineState,
    /// The content of each file-tool-edited path before the run's first edit of it, by canonical path.
    captures: HashMap<PathBuf, Capture>,
}

struct Listed {
    file: ChangedFile,
    sources: Sources,
}

#[derive(Default)]
struct Inner {
    paths: Option<Paths>,
    repository: Option<Repository>,
    scratch: Option<PathBuf>,
    run: Option<RunState>,
    records: HashMap<usize, Record>,
    /// The turn of the newest record.
    latest: Option<usize>,
}

/// Records what each run of one conversation changed.
pub struct Recorder {
    cwd: PathBuf,
    limits: Limits,
    git: Runner,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

fn warn(message: &str) {
    vivianne_core::debuglog::log("changes", message.to_owned());
}

impl Recorder {
    /// A recorder for the working folder `cwd`. An error names a limit that is not positive.
    pub fn new(cwd: &Path, limits: Limits) -> Result<Self, String> {
        limits.validate()?;
        Ok(Self {
            cwd: cwd.to_path_buf(),
            git: Runner {
                timeout: std::time::Duration::from_millis(limits.timeout_ms),
                output_max_bytes: limits.output_max_bytes,
            },
            limits,
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Opens a run: takes the baseline snapshot. Never fails the run; a snapshot that fails is warned about and the run
    /// then records nothing.
    pub async fn start(&self, turn: usize) {
        let mut inner = self.inner.lock().await;
        let baseline = match self.baseline(&mut inner).await {
            Ok(None) => BaselineState::None,
            Ok(Some(baseline)) => BaselineState::Taken(Box::new(baseline)),
            Err(error) => {
                warn(&format!("changes: {error}"));
                BaselineState::Failed
            }
        };
        inner.run = Some(RunState {
            turn,
            baseline,
            captures: HashMap::new(),
        });
    }

    /// Copies the path a file tool is about to edit, before it runs; only the run's first edit of a path captures it.
    pub async fn capture(&self, path: &Path) {
        let mut inner = self.inner.lock().await;
        let Some(scratch) = self.scratch(&mut inner) else {
            return;
        };
        let Some(run) = inner.run.as_mut() else {
            return;
        };
        let absolute = canonical_path(&if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        });
        if run.captures.contains_key(&absolute) {
            return;
        }
        match capture_file(
            &absolute,
            &scratch.join("captures"),
            self.limits.max_file_bytes,
        ) {
            Ok(Some(capture)) => {
                run.captures.insert(absolute, capture);
            }
            Ok(None) => {}
            Err(error) => warn(&format!(
                "changes: cannot capture {}: {error}",
                absolute.display()
            )),
        }
    }

    /// Closes the run: takes the end snapshot, compares, and keeps the summary. `None` when nothing changed or the
    /// record was abandoned. Never fails the run.
    pub async fn finish(&self) -> Option<ChangesSummary> {
        let mut inner = self.inner.lock().await;
        let run = inner.run.take()?;
        match self.record(&mut inner, &run).await {
            Ok(summary) => summary,
            Err(error) => {
                warn(&format!("changes: {error}"));
                None
            }
        }
    }

    /// The summary of one turn's run, while this recorder lives.
    pub async fn summary(&self, turn: usize) -> Option<ChangesSummary> {
        self.inner
            .lock()
            .await
            .records
            .get(&turn)
            .map(|r| r.summary.clone())
    }

    /// The newest run's summary.
    pub async fn latest(&self) -> Option<ChangesSummary> {
        let inner = self.inner.lock().await;
        inner
            .latest
            .and_then(|turn| inner.records.get(&turn))
            .map(|r| r.summary.clone())
    }

    /// Compares one listed file's two sides. `Ok(None)` for an unknown turn or index. An error means a read failed.
    pub async fn diff(&self, turn: usize, index: usize) -> Result<Option<FileDiff>, String> {
        let inner = self.inner.lock().await;
        let Some(record) = inner.records.get(&turn) else {
            return Ok(None);
        };
        let (Some(file), Some(sources)) =
            (record.summary.files.get(index), record.sources.get(index))
        else {
            return Ok(None);
        };
        let (path, display) = (file.path.clone(), file.display.clone());
        let (before, after) = match sources {
            Sources::Refused(Refusal::Binary) => {
                return Ok(Some(FileDiff::Binary { path, display }));
            }
            Sources::Refused(Refusal::Oversized) => {
                return Ok(Some(FileDiff::Oversized { path, display }));
            }
            Sources::Sides { before, after } => (before.clone(), after.clone()),
        };
        let (Some(before_text), Some(after_text)) = (
            self.read_side(&before).await?,
            self.read_side(&after).await?,
        ) else {
            return Ok(Some(FileDiff::Oversized { path, display }));
        };
        let comparison = compare_text(
            before_text.as_deref(),
            after_text.as_deref(),
            self.limits.diff_timeout_ms,
        );
        Ok(Some(FileDiff::Text {
            path,
            display,
            before: before_text.is_some(),
            after: after_text.is_some(),
            hunks: comparison.hunks,
            coarse: comparison.coarse,
        }))
    }

    /// The temporary folder, once something has used it.
    #[cfg(test)]
    pub(super) async fn scratch_dir(&self) -> Option<PathBuf> {
        self.inner.lock().await.scratch.clone()
    }

    /// This conversation's temporary folder, made on first use.
    #[allow(clippy::unused_self)]
    fn scratch(&self, inner: &mut Inner) -> Option<PathBuf> {
        if inner.scratch.is_none() {
            let dir =
                std::env::temp_dir().join(format!("viv-changes-{}", uuid::Uuid::new_v4().simple()));
            if let Err(error) = std::fs::create_dir_all(&dir) {
                warn(&format!("changes: cannot make a temporary folder: {error}"));
                return None;
            }
            inner.scratch = Some(dir);
        }
        inner.scratch.clone()
    }

    async fn baseline(&self, inner: &mut Inner) -> Result<Option<Baseline>, String> {
        if inner.paths.is_none() {
            inner.paths = Some(Paths {
                cwd: canonical_path(&self.cwd),
                home: canonical_path(&home_dir()),
                temporary: temporary_roots(),
            });
        }
        let Some(paths) = inner.paths.clone() else {
            return Ok(None);
        };
        let Some(repository) = self.locate(inner, &paths.cwd).await? else {
            return Ok(None);
        };
        let tree = git::snapshot_tree(&self.git, &repository.workspace).await?;
        Ok(Some(Baseline { repository, tree }))
    }

    /// The repository enclosing the working folder, located once; none keeps being looked for on each run.
    async fn locate(&self, inner: &mut Inner, cwd: &Path) -> Result<Option<Repository>, String> {
        if let Some(found) = &inner.repository {
            return Ok(Some(found.clone()));
        }
        if !git::available().await {
            return Ok(None);
        }
        let Some(scratch) = self.scratch(inner) else {
            return Ok(None);
        };
        let Some(workspace) = git::locate(&self.git, cwd, &scratch).await? else {
            return Ok(None);
        };
        let repository = Repository { workspace };
        inner.repository = Some(repository.clone());
        Ok(Some(repository))
    }

    /// One side's text, `None` for an absent file, and `Err(())`-like `None` outer for an oversized snapshot side.
    async fn read_side(&self, source: &Source) -> Result<Option<Option<String>>, String> {
        match source {
            Source::Absent => Ok(Some(None)),
            Source::File(file) => {
                let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
                Ok(Some(Some(String::from_utf8_lossy(&bytes).into_owned())))
            }
            Source::Snapshot {
                repository,
                tree,
                path,
            } => {
                let workspace = &repository.workspace;
                let Some(blob) = git::tree_blob(&self.git, workspace, tree, path).await? else {
                    return Ok(Some(None));
                };
                if blob.size > self.limits.max_file_bytes {
                    return Ok(None);
                }
                Ok(Some(Some(
                    git::blob_text(&self.git, workspace, &blob.oid, self.limits.max_file_bytes)
                        .await?,
                )))
            }
        }
    }

    async fn record(
        &self,
        inner: &mut Inner,
        run: &RunState,
    ) -> Result<Option<ChangesSummary>, String> {
        let Some(paths) = inner.paths.clone() else {
            return Ok(None);
        };
        let baseline = match &run.baseline {
            BaselineState::Failed => return Ok(None),
            BaselineState::None => None,
            BaselineState::Taken(baseline) => Some(baseline.as_ref()),
        };
        // Without a snapshot the working folder itself bounds the workspace.
        let root = baseline.map_or_else(
            || paths.cwd.clone(),
            |b| b.repository.workspace.root.clone(),
        );
        let scratch = self.scratch(inner).ok_or("no temporary folder")?;
        let mut listed: HashMap<PathBuf, Listed> = HashMap::new();
        let mut snapshot = None;
        if let Some(baseline) = baseline {
            let workspace = &baseline.repository.workspace;
            let after = git::snapshot_tree(&self.git, workspace).await?;
            snapshot = Some(SnapshotIds {
                before: baseline.tree.clone(),
                after: after.clone(),
            });
            for entry in git::diff_trees(&self.git, workspace, &baseline.tree, &after).await? {
                let absolute = root.join(&entry.path);
                let counts = Counts {
                    added: entry.added,
                    deleted: entry.deleted,
                    binary: entry.binary,
                    oversized: false,
                };
                let sources = if entry.binary {
                    Sources::Refused(Refusal::Binary)
                } else {
                    Sources::Sides {
                        before: Source::Snapshot {
                            repository: baseline.repository.clone(),
                            tree: baseline.tree.clone(),
                            path: entry.old_path.clone().unwrap_or_else(|| entry.path.clone()),
                        },
                        after: Source::Snapshot {
                            repository: baseline.repository.clone(),
                            tree: after.clone(),
                            path: entry.path.clone(),
                        },
                    }
                };
                listed.insert(
                    absolute.clone(),
                    Listed {
                        file: changed_file(&paths, &root, &absolute, &counts),
                        sources,
                    },
                );
            }
        }
        // Captured paths the snapshots do not cover are compared from their copies.
        let captured: Vec<&PathBuf> = run
            .captures
            .keys()
            .filter(|p| !listed.contains_key(*p))
            .collect();
        let work_tree_path =
            |absolute: &Path| to_posix(absolute.strip_prefix(&root).unwrap_or(absolute));
        let mut in_workspace: Vec<&PathBuf> = captured
            .iter()
            .copied()
            .filter(|p| is_inside(&root, p))
            .collect();
        if let Some(baseline) = baseline
            && !in_workspace.is_empty()
        {
            // Nested repositories and submodules are gitlinks: their contents never enter the summary.
            let links = git::gitlink_paths(&self.git, &baseline.repository.workspace).await?;
            in_workspace.retain(|p| !links.iter().any(|link| is_inside(&root.join(link), p)));
        }
        // A snapshot covers every workspace file except the ignored ones; without one, every file-tool edit counts.
        let uncovered: Vec<String> = match baseline {
            None => in_workspace.iter().map(|p| work_tree_path(p)).collect(),
            Some(baseline) => {
                let names: Vec<String> = in_workspace.iter().map(|p| work_tree_path(p)).collect();
                git::ignored_paths(&self.git, &baseline.repository.workspace, &names).await?
            }
        };
        for absolute in captured {
            // Outside the workspace, scratch files under a temporary root stay out.
            let is_uncovered = if is_inside(&root, absolute) {
                uncovered.contains(&work_tree_path(absolute))
            } else {
                !is_temporary_path(absolute, &paths.temporary)
            };
            if !is_uncovered {
                continue;
            }
            let Some(before) = run.captures.get(absolute) else {
                continue;
            };
            let Some(after) = capture_file(
                absolute,
                &scratch.join("captures"),
                self.limits.max_file_bytes,
            )
            .map_err(|e| e.to_string())?
            else {
                continue;
            };
            if same_capture(before, &after) {
                continue;
            }
            listed.insert(
                absolute.clone(),
                self.compared(&paths, &root, absolute, before, &after)?,
            );
        }
        let mut sorted: Vec<Listed> = listed.into_values().collect();
        sorted.sort_by(|a, b| a.file.display.cmp(&b.file.display));
        // An empty list after an earlier record of this turn supersedes that record.
        if sorted.is_empty() && !inner.records.contains_key(&run.turn) {
            return Ok(None);
        }
        let (total, added, deleted) = (
            sorted.len(),
            sorted.iter().map(|l| l.file.added).sum(),
            sorted.iter().map(|l| l.file.deleted).sum(),
        );
        sorted.truncate(self.limits.max_files);
        let summary = ChangesSummary {
            turn: run.turn,
            cwd: self.cwd.to_string_lossy().into_owned(),
            files: sorted.iter().map(|l| l.file.clone()).collect(),
            total,
            added,
            deleted,
            snapshot,
        };
        inner.records.insert(
            run.turn,
            Record {
                summary: summary.clone(),
                sources: sorted.into_iter().map(|l| l.sources).collect(),
            },
        );
        inner.latest = Some(run.turn);
        Ok(Some(summary))
    }

    /// The listing of a captured pair: an oversized side lists the file without counts and refuses its comparison, a
    /// binary side likewise, and two text sides carry the counts of their line comparison.
    fn compared(
        &self,
        paths: &Paths,
        root: &Path,
        absolute: &Path,
        before: &Capture,
        after: &Capture,
    ) -> Result<Listed, String> {
        let list = |counts: Counts, sources: Sources| Listed {
            file: changed_file(paths, root, absolute, &counts),
            sources,
        };
        if matches!(before, Capture::Oversized) || matches!(after, Capture::Oversized) {
            return Ok(list(
                Counts {
                    oversized: true,
                    ..Counts::default()
                },
                Sources::Refused(Refusal::Oversized),
            ));
        }
        let is_binary = |c: &Capture| matches!(c, Capture::File { binary: true, .. });
        if is_binary(before) || is_binary(after) {
            return Ok(list(
                Counts {
                    binary: true,
                    ..Counts::default()
                },
                Sources::Refused(Refusal::Binary),
            ));
        }
        let text = |c: &Capture| -> Result<Option<String>, String> {
            match c {
                Capture::File { file, .. } => Ok(Some(
                    String::from_utf8_lossy(&std::fs::read(file).map_err(|e| e.to_string())?)
                        .into_owned(),
                )),
                _ => Ok(None),
            }
        };
        let comparison = compare_text(
            text(before)?.as_deref(),
            text(after)?.as_deref(),
            self.limits.diff_timeout_ms,
        );
        let side = |c: &Capture| match c {
            Capture::File { file, .. } => Source::File(file.clone()),
            _ => Source::Absent,
        };
        Ok(list(
            Counts {
                added: comparison.added,
                deleted: comparison.deleted,
                ..Counts::default()
            },
            Sources::Sides {
                before: side(before),
                after: side(after),
            },
        ))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(dir) = self.inner.get_mut().scratch.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[derive(Default)]
struct Counts {
    added: usize,
    deleted: usize,
    binary: bool,
    oversized: bool,
}

fn changed_file(paths: &Paths, root: &Path, absolute: &Path, counts: &Counts) -> ChangedFile {
    ChangedFile {
        path: durable_path_of(absolute, &paths.cwd),
        display: display_path_of(absolute, &paths.cwd, root, &paths.home),
        added: counts.added,
        deleted: counts.deleted,
        binary: counts.binary,
        oversized: counts.oversized,
    }
}
