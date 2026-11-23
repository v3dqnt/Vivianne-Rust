//! Git for the Git tab: where a repository stands and the everyday moves (commit, sync, branches, diffs). Each
//! one runs the `git` program in the project folder. GitHub is in `github`, diff reading in `diff`.

pub mod diff;
pub mod github;
pub mod time;

use std::path::Path;

use crate::proc::quiet;

/// Runs git in `dir`: its output, or what it said on stderr as the error.
///
/// # Errors
/// When git cannot start or exits with a failure.
pub async fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = quiet("git")
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(if err.is_empty() {
        format!("git {} failed", args.join(" "))
    } else {
        err
    })
}

/// Where a repository stands right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub branch: String,
    /// `origin/<branch>`, when the branch tracks one.
    pub upstream: Option<String>,
    /// Commits here that are not upstream (to push), and upstream that are not here (to pull).
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    /// Files with merge conflicts: nothing else should happen until they are resolved.
    pub conflicted: u32,
    /// `main` or `master`, from `origin/HEAD`.
    pub default_branch: Option<String>,
    pub last_commit: Option<String>,
}

impl Summary {
    /// Files that differ from the last commit.
    #[must_use]
    pub fn changed(&self) -> u32 {
        self.staged + self.unstaged + self.untracked + self.conflicted
    }

    /// What needs a look, for the title bar's Git tab: red for conflicts, amber for unfinished work.
    #[must_use]
    pub fn attention(&self, checks_failing: bool) -> Option<Attention> {
        if self.conflicted > 0 || checks_failing {
            Some(Attention::Red)
        } else if self.changed() > 0 || self.ahead > 0 || self.behind > 0 {
            Some(Attention::Amber)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    Red,
    Amber,
}

/// Reads `git status --porcelain=v2 --branch`.
#[must_use]
pub fn parse_status(text: &str) -> Summary {
    let mut s = Summary::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            rest.clone_into(&mut s.branch);
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    s.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    s.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("? ") {
            s.untracked += 1;
        } else if line.starts_with("u ") {
            s.conflicted += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // `1 XY ...`: X is the index (staged), Y the work tree.
            let mut xy = line.chars().skip(2);
            if xy.next().is_some_and(|c| c != '.') {
                s.staged += 1;
            }
            if xy.next().is_some_and(|c| c != '.') {
                s.unstaged += 1;
            }
        }
    }
    s
}

/// The repository's state, or `None` when the folder is not in one.
pub async fn summary(dir: &str) -> Option<Summary> {
    let text = git(dir, &["status", "--porcelain=v2", "--branch"])
        .await
        .ok()?;
    let mut s = parse_status(&text);
    s.default_branch = git(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    .ok()
    .map(|r| r.trim_start_matches("origin/").to_owned());
    s.last_commit = git(dir, &["log", "-1", "--format=%h %s (%cr)"])
        .await
        .ok()
        .filter(|l| !l.is_empty());
    Some(s)
}

/// The `origin` remote, and the GitHub repository it names when it is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub url: String,
    /// `owner/repo`.
    pub github_repo: Option<String>,
}

impl Remote {
    /// The address to open in a browser.
    #[must_use]
    pub fn github_url(&self) -> Option<String> {
        self.github_repo
            .as_ref()
            .map(|r| format!("https://github.com/{r}"))
    }
}

pub async fn remote(dir: &str) -> Option<Remote> {
    let url = git(dir, &["remote", "get-url", "origin"])
        .await
        .ok()
        .filter(|u| !u.is_empty())?;
    let github_repo = github::parse_remote(&url);
    Some(Remote { url, github_repo })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    /// ISO 8601.
    pub date: String,
    pub message: String,
}

/// The newest commits; empty when the folder is not a repository.
pub async fn log(dir: &str, limit: u32) -> Vec<Commit> {
    let n = format!("-n{}", limit.clamp(1, 500));
    let Ok(text) = git(
        dir,
        &[
            "log",
            &n,
            "--date=iso-strict",
            "--pretty=format:%H%x1f%an%x1f%ad%x1f%s",
        ],
    )
    .await
    else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '\u{1f}');
            Some(Commit {
                hash: parts.next()?.to_owned(),
                author: parts.next()?.to_owned(),
                date: parts.next()?.to_owned(),
                message: parts.next().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
}

/// Local and remote-tracking branches.
pub async fn branches(dir: &str) -> Vec<Branch> {
    let Ok(text) = git(
        dir,
        &["branch", "-a", "--format=%(HEAD)%09%(refname:short)"],
    )
    .await
    else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (head, name) = line.split_once('\t')?;
            // `origin/HEAD -> origin/main` is a pointer, not a branch.
            if name.contains("HEAD ->") || name.ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                is_current: head == "*",
                is_remote: name.starts_with("remotes/") || name.starts_with("origin/"),
                name: name.to_owned(),
            })
        })
        .collect()
}

/// One file with its change counts, for the review list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    /// `M`, or `??` when the file is new to git.
    pub status: String,
}

/// The repository's top folder: paths git reports are relative to it, wherever it was asked from.
async fn top(dir: &str) -> String {
    git(dir, &["rev-parse", "--show-toplevel"])
        .await
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| dir.to_owned())
}

/// Added and removed lines per changed file. Untracked files are counted by reading them, so a new file does
/// not show as `+0 -0`.
pub async fn diff_stat(dir: &str, staged: bool) -> Vec<FileChange> {
    let top = top(dir).await;
    // No rename detection: numstat would print `a => b`, a path nothing else can find.
    let mut args = vec!["diff", "--no-renames", "--numstat"];
    if staged {
        args.push("--cached");
    }
    let mut changes: Vec<FileChange> = git(&top, &args)
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let (added, removed, path) = (parts.next()?, parts.next()?, parts.next()?);
            // "-" is a binary file.
            Some(FileChange {
                path: path.to_owned(),
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
                status: "M".to_owned(),
            })
        })
        .collect();
    if !staged {
        let others = git(&top, &["ls-files", "--others", "--exclude-standard"])
            .await
            .unwrap_or_default();
        for path in others.lines().filter(|l| !l.trim().is_empty()) {
            let added = std::fs::read_to_string(Path::new(&top).join(path))
                .map_or(0, |c| u32::try_from(c.lines().count()).unwrap_or(u32::MAX));
            changes.push(FileChange {
                path: path.to_owned(),
                added,
                removed: 0,
                status: "??".to_owned(),
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

/// The diff of one file. A file git has no diff for (new) is shown whole as additions. `context` is the
/// lines around each change; `ignore_whitespace` leaves out whitespace-only changes.
pub async fn diff_file(
    dir: &str,
    file: &str,
    staged: bool,
    context: Option<u32>,
    ignore_whitespace: bool,
) -> String {
    let top = top(dir).await;
    let mut args: Vec<String> = vec!["diff".into(), "--no-renames".into()];
    if let Some(c) = context {
        args.push(format!("-U{c}"));
    }
    if ignore_whitespace {
        args.push("-w".into());
    }
    if staged {
        args.push("--cached".into());
    }
    args.extend(["--".into(), file.to_owned()]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let diff = git(&top, &args).await.unwrap_or_default();
    if !diff.trim().is_empty() {
        return diff;
    }
    match std::fs::read_to_string(Path::new(&top).join(file)) {
        Ok(content) => {
            let count = content.lines().count();
            let body = content.lines().fold(String::new(), |mut body, l| {
                body.push('+');
                body.push_str(l);
                body.push('\n');
                body
            });
            format!("--- /dev/null\n+++ b/{file}\n@@ -0,0 +1,{count} @@\n{body}")
        }
        Err(_) => String::new(),
    }
}

/// Commits the named files, or everything that changed. Returns the new commit as `hash subject`.
///
/// # Errors
/// An empty message, or git's own refusal.
pub async fn commit(dir: &str, message: &str, files: Option<&[String]>) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("A commit needs a message.".to_owned());
    }
    match files {
        Some(files) if !files.is_empty() => {
            let mut args = vec!["add", "--"];
            args.extend(files.iter().map(String::as_str));
            git(dir, &args).await?;
        }
        _ => {
            git(dir, &["add", "-A"]).await?;
        }
    }
    git(dir, &["commit", "-m", message]).await?;
    git(dir, &["log", "-1", "--format=%h %s"]).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Pull,
    Push,
    Both,
}

/// Brings the branch up to date with its upstream, then sends what is new. Pulls only fast-forward: a branch that
/// has diverged says so instead of merging behind your back.
///
/// # Errors
/// Conflicts, a diverged branch, or git's own refusal.
pub async fn sync(dir: &str, action: Direction) -> Result<String, String> {
    let s = summary(dir)
        .await
        .ok_or("This folder isn't a git repository.")?;
    if s.conflicted > 0 {
        return Err("Resolve the merge conflicts first.".to_owned());
    }
    let mut done = Vec::new();
    if matches!(action, Direction::Pull | Direction::Both) && s.upstream.is_some() {
        git(dir, &["pull", "--ff-only"]).await.map_err(|e| {
            if e.contains("Not possible to fast-forward") || e.contains("diverg") {
                "This branch and its upstream have both moved. Rebase or merge them (the agent can do it), then sync.".to_owned()
            } else {
                e
            }
        })?;
        done.push("pulled");
    }
    if matches!(action, Direction::Push | Direction::Both) {
        if s.upstream.is_some() {
            git(dir, &["push"]).await?;
        } else {
            git(dir, &["push", "-u", "origin", "HEAD"]).await?;
        }
        done.push("pushed");
    }
    Ok(if done.is_empty() {
        "Nothing to do.".to_owned()
    } else {
        done.join(" and ")
    })
}

/// The folder that has `branch` checked out, when some worktree does.
async fn worktree_of(dir: &str, branch: &str) -> Option<String> {
    let list = git(dir, &["worktree", "list", "--porcelain"]).await.ok()?;
    let want = format!("branch refs/heads/{branch}");
    let mut current = None;
    for line in list.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path);
        } else if line == want {
            return current.map(|p| p.replace('/', std::path::MAIN_SEPARATOR_STR));
        }
    }
    None
}

/// Checks out a branch, or a pull request's head (`pr`), and says which folder it is checked out in. A branch
/// some worktree already has is left there; one only on `origin` is fetched and tracked; a pull request is
/// fetched from `pull/<n>/head`, so one from a fork works too.
///
/// # Errors
/// An unknown branch, or changes in the way.
pub async fn checkout(dir: &str, branch: &str, pr: Option<u64>) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() || branch.starts_with('-') {
        return Err("No branch to check out.".to_owned());
    }
    if let Some(found) = worktree_of(dir, branch).await {
        return Ok(found);
    }
    let local = git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .is_ok();
    if let Some(n) = pr {
        git(
            dir,
            &[
                "fetch",
                "origin",
                &format!("+pull/{n}/head:refs/heads/{branch}"),
            ],
        )
        .await?;
    } else if !local {
        git(dir, &["fetch", "origin", branch])
            .await
            .map_err(|_| format!("There's no branch '{branch}' here or on origin."))?;
        git(
            dir,
            &["branch", "--track", branch, &format!("origin/{branch}")],
        )
        .await?;
    }
    git(dir, &["checkout", branch]).await?;
    Ok(dir.to_owned())
}

/// A new branch off the current commit, switched to.
///
/// # Errors
/// An invalid or taken name.
pub async fn create_branch(dir: &str, name: &str) -> Result<(), String> {
    git(dir, &["checkout", "-b", name.trim()]).await.map(|_| ())
}

const COMMIT_PROMPT: &str = "Write a git commit message for the diff below. First line: an imperative summary under 72 characters, no trailing period, no prefix like 'feat:' unless the repository's recent messages use them. Then a blank line and, only if the change needs it, a few short lines on why. Reply with the message only.";
const PR_PROMPT: &str = "Write a pull request for the branch below, from its commits and diff stat. Reply as JSON: {\"title\": \"...\", \"body\": \"...\"}. Title: imperative, under 72 characters. Body in Markdown: ## What (2-5 bullets), ## Why (1-3 sentences), ## Testing (what was run or what should be checked). No preamble.";

/// What to ask a model to write a commit message or a pull request: its instructions and the material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestInput {
    pub system: &'static str,
    pub user: String,
    pub is_pr: bool,
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let end = (0..=max)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    format!("{}\n… (cut)", &text[..end])
}

/// The material for a commit message (the staged diff, else everything) or, with `base`, a pull request.
///
/// # Errors
/// Not a repository, or nothing changed.
pub async fn suggest_input(
    dir: &str,
    pr: bool,
    base: Option<&str>,
) -> Result<SuggestInput, String> {
    if pr {
        let s = summary(dir)
            .await
            .ok_or("This folder isn't a git repository.")?;
        let base = base
            .map(str::to_owned)
            .or(s.default_branch.clone())
            .unwrap_or_else(|| "main".to_owned());
        let log = git(
            dir,
            &["log", "--format=- %s%n%b", &format!("origin/{base}..HEAD")],
        )
        .await
        .unwrap_or_default();
        let stat = git(dir, &["diff", "--stat", &format!("origin/{base}...HEAD")])
            .await
            .unwrap_or_default();
        let user = format!(
            "Branch: {}\nInto: {base}\n\nCommits:\n{}\n\nDiff stat:\n{}",
            s.branch,
            clip(&log, 6000),
            clip(&stat, 3000)
        );
        return Ok(SuggestInput {
            system: PR_PROMPT,
            user,
            is_pr: true,
        });
    }
    let staged = git(dir, &["diff", "--cached"]).await.unwrap_or_default();
    let diff = if staged.trim().is_empty() {
        git(dir, &["diff", "HEAD"]).await.unwrap_or_default()
    } else {
        staged
    };
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard"])
        .await
        .unwrap_or_default();
    if diff.trim().is_empty() && untracked.trim().is_empty() {
        return Err("Nothing has changed.".to_owned());
    }
    let recent = git(dir, &["log", "-8", "--format=%s"])
        .await
        .unwrap_or_default();
    let user = format!(
        "Recent messages here:\n{recent}\n\nNew files:\n{}\n\nDiff:\n{}",
        clip(&untracked, 2000),
        clip(&diff, 12000)
    );
    Ok(SuggestInput {
        system: COMMIT_PROMPT,
        user,
        is_pr: false,
    })
}

/// A model's reply as a title and a body: JSON for a pull request, else the first line and the rest.
#[must_use]
pub fn parse_suggestion(reply: &str, is_pr: bool, fallback_title: &str) -> (String, String) {
    if is_pr {
        let from = reply.find('{');
        let to = reply.rfind('}');
        if let (Some(from), Some(to)) = (from, to)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply[from..=to])
        {
            let text = |k: &str| {
                value
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            };
            return (text("title"), text("body"));
        }
        return (fallback_title.to_owned(), reply.trim().to_owned());
    }
    let reply = reply.trim().trim_matches('`').trim();
    match reply.split_once("\n\n") {
        Some((title, body)) => (title.trim().to_owned(), body.trim().to_owned()),
        None => (reply.to_owned(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_is_counted() {
        let text = "# branch.oid abc\n# branch.head feature/x\n# branch.upstream origin/feature/x\n# branch.ab +2 -1\n1 M. N... 100644 100644 100644 a b src/a.rs\n1 .M N... 100644 100644 100644 a b src/b.rs\n1 MM N... 100644 100644 100644 a b src/c.rs\nu UU N... 100644 100644 100644 100644 a b c src/d.rs\n? new.txt\n? other.txt";
        let s = parse_status(text);
        assert_eq!(s.branch, "feature/x");
        assert_eq!(s.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicted),
            (2, 2, 2, 1)
        );
        assert_eq!(s.changed(), 7);
        assert_eq!(s.attention(false), Some(Attention::Red));
    }

    #[test]
    fn attention_follows_the_work_left() {
        let clean = Summary::default();
        assert_eq!(clean.attention(false), None);
        assert_eq!(clean.attention(true), Some(Attention::Red));
        assert_eq!(
            Summary {
                ahead: 1,
                ..Summary::default()
            }
            .attention(false),
            Some(Attention::Amber)
        );
    }

    #[test]
    fn a_pull_request_reply_is_read_from_its_json() {
        let (title, body) = parse_suggestion(
            "Here:\n{\"title\": \"Add x\", \"body\": \"## What\\n- x\"}",
            true,
            "branch",
        );
        assert_eq!((title.as_str(), body.as_str()), ("Add x", "## What\n- x"));
        let (title, _) = parse_suggestion("not json", true, "branch");
        assert_eq!(title, "branch");
    }

    #[test]
    fn a_commit_reply_splits_into_a_summary_and_details() {
        let (title, body) = parse_suggestion(
            "`Fix the login timeout\n\nThe cookie expired early.`",
            false,
            "",
        );
        assert_eq!(
            (title.as_str(), body.as_str()),
            ("Fix the login timeout", "The cookie expired early.")
        );
    }

    #[tokio::test]
    async fn a_repository_commits_diffs_and_syncs_nothing_without_a_remote() {
        let dir = std::env::temp_dir().join(format!("viv-git-tab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let d = dir.to_string_lossy().into_owned();
        let _ = git(&d, &["init", "-q", "-b", "main"]).await;
        let _ = git(&d, &["config", "user.email", "t@t"]).await;
        let _ = git(&d, &["config", "user.name", "t"]).await;
        std::fs::write(dir.join("a.txt"), "one\n").unwrap_or_default();
        assert_eq!(
            summary(&d).await.map(|s| (s.branch, s.untracked)),
            Some(("main".to_owned(), 1))
        );
        let stat = diff_stat(&d, false).await;
        assert_eq!(
            stat,
            [FileChange {
                path: "a.txt".into(),
                added: 1,
                removed: 0,
                status: "??".into()
            }]
        );
        assert!(
            diff_file(&d, "a.txt", false, None, false)
                .await
                .contains("+one"),
            "a new file reads as additions"
        );
        let made = commit(&d, "Add a", None).await;
        assert!(
            made.as_deref().is_ok_and(|m| m.ends_with("Add a")),
            "{made:?}"
        );
        assert_eq!(summary(&d).await.map(|s| s.changed()), Some(0));
        assert!(
            commit(&d, "  ", None).await.is_err(),
            "a message is required"
        );
        std::fs::write(dir.join("a.txt"), "two\n").unwrap_or_default();
        let diff = diff_file(&d, "a.txt", false, Some(3), false).await;
        assert!(diff.contains("-one") && diff.contains("+two"), "{diff}");
        assert_eq!(log(&d, 5).await.len(), 1);
        // A renamed file is listed under its new name, which `diff_file` can then find.
        let _ = git(&d, &["mv", "a.txt", "b.txt"]).await;
        let renamed = diff_stat(&d, true).await;
        assert!(
            renamed.iter().all(|c| !c.path.contains("=>")),
            "{renamed:?}"
        );
        assert_eq!(
            branches(&d).await.iter().filter(|b| b.is_current).count(),
            1
        );
        assert!(
            sync(&d, Direction::Pull)
                .await
                .is_ok_and(|r| r == "Nothing to do.")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
