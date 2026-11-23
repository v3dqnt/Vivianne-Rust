//! What GitHub says about the current branch, kept fresh for the title bar's Git tab and the page's header: this
//! branch's pull request with its checks, the open pull requests and issues, and the workflows whose latest run
//! failed.

use std::collections::HashSet;

use serde_json::Value;

use super::ui::{github, list, n, s};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    None,
    Running,
    Passing,
    Failing,
}

/// The pull request of the current branch, with its checks rolled up.
#[derive(Debug, Clone)]
pub struct BranchPr {
    pub number: i64,
    pub state: PrState,
    pub draft: bool,
    pub url: String,
    pub checks: Checks,
    pub failing: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Github {
    /// `owner/name`; `None` when there is no GitHub remote or no credentials.
    pub repo: Option<String>,
    pub pr: Option<BranchPr>,
    pub open_prs: Option<u32>,
    pub open_issues: Option<u32>,
    pub failing_runs: Option<u32>,
}

fn count(v: &Value) -> u32 {
    u32::try_from(list(v).len()).unwrap_or(u32::MAX)
}

/// Rolls a pull request's check runs up into one word and the names of what failed.
#[must_use]
pub fn roll_up(check_runs: &[Value]) -> (Checks, Vec<String>) {
    let failing: Vec<String> = check_runs
        .iter()
        .filter(|c| matches!(s(c, "conclusion"), "failure" | "timed_out"))
        .map(|c| s(c, "name").to_owned())
        .collect();
    let running = check_runs.iter().any(|c| s(c, "status") != "completed");
    let checks = if !failing.is_empty() {
        Checks::Failing
    } else if running {
        Checks::Running
    } else if check_runs.is_empty() {
        Checks::None
    } else {
        Checks::Passing
    };
    (checks, failing)
}

/// How many workflows have their latest run on this branch failed. Runs are newest first.
#[must_use]
pub fn failing_workflows(runs: &[Value]) -> u32 {
    let mut seen = HashSet::new();
    let mut failing = 0;
    for run in runs {
        if seen.insert(s(run, "name").to_owned()) && s(run, "conclusion") == "failure" {
            failing += 1;
        }
    }
    failing
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Reads the state from GitHub. Without a remote or credentials the answer is empty and the local state still
/// shows.
pub async fn fetch(root: String, branch: String) -> Github {
    let get = |path: String| github(root.clone(), path, "GET", None, false);
    let Ok(repo) = vivianne_tools::git::github::repo(&root).await else {
        return Github::default();
    };
    let owner = repo.split('/').next().unwrap_or("").to_owned();
    let branch = encode(&branch);
    let (prs, open, info, runs) = futures::join!(
        get(format!(
            "/repos/{{repo}}/pulls?head={owner}:{branch}&state=all&per_page=1"
        )),
        get("/repos/{repo}/pulls?state=open&per_page=100".to_owned()),
        get("/repos/{repo}".to_owned()),
        get(format!(
            "/repos/{{repo}}/actions/runs?branch={branch}&per_page=10"
        )),
    );
    let (Ok(prs), Ok(open)) = (prs, open) else {
        return Github {
            repo: Some(repo),
            ..Github::default()
        };
    };
    let mut pr = None;
    if let Some(p) = list(&prs).first() {
        let sha = p.get("head").map_or("", |h| s(h, "sha")).to_owned();
        let checks = get(format!(
            "/repos/{{repo}}/commits/{sha}/check-runs?per_page=100"
        ))
        .await
        .unwrap_or(Value::Null);
        let (rolled, failing) = roll_up(list(checks.get("check_runs").unwrap_or(&Value::Null)));
        pr = Some(BranchPr {
            number: n(p, "number"),
            state: if !s(p, "merged_at").is_empty() {
                PrState::Merged
            } else if s(p, "state") == "closed" {
                PrState::Closed
            } else {
                PrState::Open
            },
            draft: p.get("draft").and_then(Value::as_bool).unwrap_or(false),
            url: s(p, "html_url").to_owned(),
            checks: rolled,
            failing,
        });
    }
    let open_prs = count(&open);
    // GitHub counts pull requests as issues too.
    let open_issues = info
        .as_ref()
        .map_or(0, |i| u32::try_from(n(i, "open_issues_count")).unwrap_or(0))
        .saturating_sub(open_prs);
    let failing_runs = runs.as_ref().map_or(0, |r| {
        failing_workflows(list(r.get("workflow_runs").unwrap_or(&Value::Null)))
    });
    Github {
        repo: Some(repo),
        pr,
        open_prs: Some(open_prs),
        open_issues: Some(open_issues),
        failing_runs: Some(failing_runs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checks_roll_up_to_the_worst_state() {
        let ok = json!({ "name": "build", "status": "completed", "conclusion": "success" });
        let bad = json!({ "name": "test", "status": "completed", "conclusion": "failure" });
        let busy = json!({ "name": "lint", "status": "in_progress" });
        assert_eq!(roll_up(std::slice::from_ref(&ok)).0, Checks::Passing);
        assert_eq!(roll_up(&[ok.clone(), busy.clone()]).0, Checks::Running);
        assert_eq!(
            roll_up(&[ok, busy, bad]),
            (Checks::Failing, vec!["test".to_owned()])
        );
        assert_eq!(roll_up(&[]).0, Checks::None);
    }

    #[test]
    fn only_a_workflows_latest_run_counts() {
        let runs = [
            json!({ "name": "CI", "conclusion": "success" }),
            json!({ "name": "CI", "conclusion": "failure" }),
            json!({ "name": "Release", "conclusion": "failure" }),
        ];
        assert_eq!(failing_workflows(&runs), 1);
    }

    #[test]
    fn a_branch_name_is_safe_in_a_url() {
        assert_eq!(encode("feature/x y"), "feature%2Fx%20y");
    }
}
