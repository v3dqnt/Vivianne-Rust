//! Git working-tree snapshots, tree diffs and ignore checks. Snapshots are written through a private index into a private
//! object folder, with the repository's own object store attached read-only as an alternate: the repository's index,
//! objects, work tree and refs stay untouched, and the user's earlier uncommitted changes never enter a summary.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::numstat::{self, Entry};
use super::paths::{canonical_path, is_inside, to_posix};

/// Retained stderr tail for diagnostics.
const STDERR_TAIL_BYTES: u64 = 16 * 1024;

/// Settled git command facts; a nonzero exit is a result, not an error.
#[derive(Debug)]
pub struct RunResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Stdout exceeded the cap and was cut.
    pub truncated: bool,
}

/// Per-command facts.
#[derive(Debug, Default)]
pub struct RunOptions<'a> {
    pub env: Vec<(String, String)>,
    pub stdin: Option<String>,
    /// Replaces the runner's output cap for this command.
    pub max_bytes: Option<usize>,
    pub cwd: Option<&'a Path>,
}

/// Runs git with a timeout, bounded output, and no prompts.
#[derive(Debug, Clone)]
pub struct Runner {
    pub timeout: Duration,
    pub output_max_bytes: usize,
}

impl Runner {
    /// Runs `git <args>` to completion in `cwd`. An error means a timeout or a failure to start.
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        options: RunOptions<'_>,
    ) -> Result<RunResult, String> {
        let cap = options.max_bytes.unwrap_or(self.output_max_bytes);
        let mut cmd = vivianne_tools::proc::quiet("git");
        cmd.args(args)
            .current_dir(options.cwd.unwrap_or(cwd))
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .stdin(if options.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &options.env {
            cmd.env(key, value);
        }
        let what = args.join(" ");
        let work = async {
            let mut child = cmd.spawn().map_err(|e| format!("git {what}: {e}"))?;
            if let (Some(text), Some(mut stdin)) = (options.stdin, child.stdin.take()) {
                // A command that exits early closes its stdin; that is its answer, not our failure.
                let _ = stdin.write_all(text.as_bytes()).await;
            }
            let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
            let read_out = async {
                let mut buf = Vec::new();
                if let Some(out) = out.as_mut() {
                    out.take(cap as u64 + 1)
                        .read_to_end(&mut buf)
                        .await
                        .map_err(|e| e.to_string())?;
                    // Drain what is over the cap so the child is not left blocked on a full pipe.
                    let _ = tokio::io::copy(out, &mut tokio::io::sink()).await;
                }
                Ok::<_, String>(buf)
            };
            let read_err = async {
                let mut buf = Vec::new();
                if let Some(err) = err.as_mut() {
                    let _ = err.take(STDERR_TAIL_BYTES).read_to_end(&mut buf).await;
                    let _ = tokio::io::copy(err, &mut tokio::io::sink()).await;
                }
                buf
            };
            let (out_bytes, err_bytes) = tokio::join!(read_out, read_err);
            let status = child.wait().await.map_err(|e| format!("git {what}: {e}"))?;
            let mut out_bytes = out_bytes?;
            let truncated = out_bytes.len() > cap;
            out_bytes.truncate(cap);
            Ok(RunResult {
                exit_code: status.code(),
                stdout: String::from_utf8_lossy(&out_bytes).into_owned(),
                stderr: String::from_utf8_lossy(&err_bytes).into_owned(),
                truncated,
            })
        };
        tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| format!("git {what} timed out after {}ms", self.timeout.as_millis()))?
    }
}

/// A failed command with its stderr.
fn ok(result: RunResult, what: &str) -> Result<RunResult, String> {
    if result.exit_code == Some(0) {
        Ok(result)
    } else {
        Err(format!("{what} failed: {}", result.stderr.trim()))
    }
}

/// The repository enclosing a working folder and the private folder its snapshots write to.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The repository's top-level folder: the root every diff path is relative to.
    pub root: PathBuf,
    /// The git folder holding the repository's index.
    pub git_dir: PathBuf,
    /// The private folder for snapshot objects and scratch indexes.
    pub scratch: PathBuf,
    /// Routes object writes to the private store and object reads through the repository's.
    pub env: Vec<(String, String)>,
    /// Work-tree paths a snapshot must skip: the private folder when a temporary root lies inside the work tree.
    pub excludes: Vec<String>,
}

/// Finds the repository enclosing `cwd` and prepares the private folder for its snapshots. `Ok(None)` outside any
/// repository; any other git failure is an error.
pub async fn locate(git: &Runner, cwd: &Path, scratch: &Path) -> Result<Option<Workspace>, String> {
    let found = git
        .run(
            cwd,
            &[
                "rev-parse",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-path",
                "objects",
            ],
            RunOptions::default(),
        )
        .await?;
    if found.exit_code == Some(128) && found.stderr.to_lowercase().contains("not a git repository")
    {
        return Ok(None);
    }
    let found = ok(found, "git rev-parse")?;
    let mut lines = found.stdout.lines().map(|line| {
        let path = PathBuf::from(line);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    });
    let (Some(root), Some(git_dir), Some(repository_objects)) =
        (lines.next(), lines.next(), lines.next())
    else {
        return Err("git rev-parse printed too little".into());
    };
    // git reports the canonical root; compare the private folder in the same spelling.
    let scratch = canonical_path(scratch);
    let objects = scratch.join("objects");
    std::fs::create_dir_all(&objects).map_err(|e| e.to_string())?;
    let root = canonical_path(&root);
    let excludes = if is_inside(&root, &scratch) {
        scratch
            .strip_prefix(&root)
            .map(|p| vec![to_posix(p)])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let env = vec![
        (
            "GIT_OBJECT_DIRECTORY".to_owned(),
            objects.to_string_lossy().into_owned(),
        ),
        (
            "GIT_ALTERNATE_OBJECT_DIRECTORIES".to_owned(),
            repository_objects.to_string_lossy().into_owned(),
        ),
    ];
    Ok(Some(Workspace {
        root,
        git_dir,
        scratch,
        env,
        excludes,
    }))
}

/// Writes the whole work tree, untracked and modified files included but not ignored ones, as a tree object through a
/// private index seeded from the repository's. Returns the tree id.
pub async fn snapshot_tree(git: &Runner, workspace: &Workspace) -> Result<String, String> {
    let scratch = workspace
        .scratch
        .join(format!("index-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let result = async {
        let index = scratch.join("index");
        // A repository without an index yet (a fresh `git init`) starts from nothing.
        if let Err(e) = std::fs::copy(workspace.git_dir.join("index"), &index)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.to_string());
        }
        let mut env = workspace.env.clone();
        env.push((
            "GIT_INDEX_FILE".to_owned(),
            index.to_string_lossy().into_owned(),
        ));
        let exclude: Vec<String> = workspace
            .excludes
            .iter()
            .map(|p| format!(":(exclude){p}"))
            .collect();
        let mut args = vec!["add", "--all", "--ignore-errors"];
        if !exclude.is_empty() {
            args.extend(["--", "."]);
            args.extend(exclude.iter().map(String::as_str));
        }
        // `--ignore-errors` skips unreadable files and says so with exit code 1; the index is still complete.
        let added = git
            .run(
                &workspace.root,
                &args,
                RunOptions {
                    env: env.clone(),
                    ..RunOptions::default()
                },
            )
            .await?;
        if added.exit_code != Some(1) {
            ok(added, &format!("git add in {}", workspace.root.display()))?;
        }
        let tree = git
            .run(
                &workspace.root,
                &["write-tree"],
                RunOptions {
                    env,
                    ..RunOptions::default()
                },
            )
            .await?;
        Ok(ok(tree, "git write-tree")?.stdout.trim().to_owned())
    }
    .await;
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// A blob one snapshot tree holds at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeBlob {
    pub oid: String,
    pub size: usize,
}

fn literal_env(workspace: &Workspace) -> Vec<(String, String)> {
    let mut env = workspace.env.clone();
    env.push(("GIT_LITERAL_PATHSPECS".to_owned(), "1".to_owned()));
    env
}

/// The blob a snapshot tree holds at one path; `None` when it holds nothing there, or a gitlink or a tree.
pub async fn tree_blob(
    git: &Runner,
    workspace: &Workspace,
    tree: &str,
    path: &str,
) -> Result<Option<TreeBlob>, String> {
    // The path is a pathspec; literal matching keeps `*`, `?` and `[` in a file name from selecting another entry.
    let result = ok(
        git.run(
            &workspace.root,
            &["ls-tree", "-z", "-l", tree, "--", path],
            RunOptions {
                env: literal_env(workspace),
                ..RunOptions::default()
            },
        )
        .await?,
        "git ls-tree",
    )?;
    let entry = result.stdout.split('\0').next().unwrap_or_default();
    let Some((meta, _name)) = entry.split_once('\t') else {
        return Ok(None);
    };
    let fields: Vec<&str> = meta.split_whitespace().collect();
    let [_mode, kind, oid, size] = fields.as_slice() else {
        return Ok(None);
    };
    if *kind != "blob" {
        return Ok(None);
    }
    Ok(size.parse().ok().map(|size| TreeBlob {
        oid: (*oid).to_owned(),
        size,
    }))
}

/// The text of one blob whose size [`tree_blob`] reported within the cap.
pub async fn blob_text(
    git: &Runner,
    workspace: &Workspace,
    oid: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let result = ok(
        git.run(
            &workspace.root,
            &["cat-file", "blob", oid],
            RunOptions {
                env: workspace.env.clone(),
                max_bytes: Some(max_bytes),
                ..RunOptions::default()
            },
        )
        .await?,
        "git cat-file",
    )?;
    if result.truncated {
        return Err(format!("blob {oid} exceeds {max_bytes} bytes"));
    }
    Ok(result.stdout)
}

/// Per-file line counts between two snapshot trees, with renames detected.
pub async fn diff_trees(
    git: &Runner,
    workspace: &Workspace,
    before: &str,
    after: &str,
) -> Result<Vec<Entry>, String> {
    if before == after {
        return Ok(Vec::new());
    }
    let result = ok(
        git.run(
            &workspace.root,
            &["diff-tree", "-r", "-M", "-z", "--numstat", before, after],
            RunOptions {
                env: workspace.env.clone(),
                ..RunOptions::default()
            },
        )
        .await?,
        "git diff-tree",
    )?;
    if result.truncated {
        return Err("git diff-tree output exceeded the configured cap".into());
    }
    numstat::parse(&result.stdout)
}

/// Work-tree folders the index records as gitlinks (nested repositories and submodules): snapshots never descend into
/// them and `check-ignore` refuses to classify them.
pub async fn gitlink_paths(git: &Runner, workspace: &Workspace) -> Result<Vec<String>, String> {
    let result = ok(
        git.run(
            &workspace.root,
            &["ls-files", "-z", "--stage"],
            RunOptions {
                env: workspace.env.clone(),
                ..RunOptions::default()
            },
        )
        .await?,
        "git ls-files",
    )?;
    Ok(result
        .stdout
        .split('\0')
        .filter(|entry| entry.starts_with("160000 "))
        .filter_map(|entry| entry.split_once('\t').map(|(_, path)| path.to_owned()))
        .collect())
}

/// The members of `paths` the repository ignores. Tracked files are never reported, so a tracked file matching an ignore
/// pattern still counts as covered by the snapshots.
pub async fn ignored_paths(
    git: &Runner,
    workspace: &Workspace,
    paths: &[String],
) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let stdin = format!("{}\0", paths.join("\0"));
    let result = git
        .run(
            &workspace.root,
            &["check-ignore", "-z", "--stdin"],
            RunOptions {
                env: workspace.env.clone(),
                stdin: Some(stdin),
                ..RunOptions::default()
            },
        )
        .await?;
    if result.exit_code == Some(1) {
        return Ok(Vec::new());
    }
    Ok(ok(result, "git check-ignore")?
        .stdout
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Whether a git executable runs here. One that does not (the macOS developer-tools stub fails until the tools are
/// installed) counts as absent, and the recorder then lists file-tool edits only.
pub async fn available() -> bool {
    let probe = vivianne_tools::proc::quiet("git")
        .arg("--version")
        .stdin(Stdio::null())
        .output();
    matches!(tokio::time::timeout(Duration::from_secs(5), probe).await, Ok(Ok(out)) if out.status.success())
}
