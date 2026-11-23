//! The run ledger: the durable record of a task meant to outlast the session.
//!
//! A chat turn lives in memory, so a run that is cut off by the app closing is simply gone. The ledger is what
//! says otherwise: what the task is for, what it may spend, what it has spent, what it has finished, and whether it
//! is running, waiting out a rate limit, paused or done. It is kept on disk, rewritten after every change, and
//! read back by a relaunched app to offer the work again.
//!
//! One ledger per thread, `~/.viv/agent/long-tasks/<thread>.json`, in the old app's format, so what the old app
//! wrote can still be read. Every change is read, changed and written back under one lock, so the run's own
//! bookkeeping and the user's pause or raised cap never overwrite each other from a stale copy.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a task stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Working, or about to.
    Running,
    /// Waiting out a rate limit or an outage; goes on by itself at `wait_until`.
    Waiting,
    /// Stopped on a cap, an error or the user's pause. Goes on when asked.
    Paused,
    /// The agent could not make progress and needs direction.
    Blocked,
    /// The goal is complete.
    Done,
    /// Ended by the user.
    Stopped,
}

impl Status {
    /// Work a relaunched app should offer to take up again.
    #[must_use]
    pub fn resumes_on_launch(self) -> bool {
        matches!(self, Self::Running | Self::Waiting)
    }

    /// Nothing more will happen to it.
    #[must_use]
    pub fn is_over(self) -> bool {
        matches!(self, Self::Done | Self::Stopped)
    }
}

/// One finished item of the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Milestone {
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    pub at: u64,
    /// Spend and time since the one before, for a checkpoint card.
    #[serde(default)]
    pub spent_usd: f64,
    #[serde(default)]
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    /// The thread it belongs to.
    pub key: String,
    #[serde(default)]
    pub thread: String,
    /// The project folder, so a relaunched app offers the task in the project it was in.
    #[serde(default)]
    pub workspace: Option<String>,
    pub objective: String,
    pub status: Status,
    #[serde(default)]
    pub cap_usd: Option<f64>,
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    pub started_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub spent_usd: f64,
    /// Tokens read (cache reads included) and written over the task.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub steps: u64,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub wait_until: Option<u64>,
    #[serde(default)]
    pub status_reason: Option<String>,
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    /// The plan as last written: `(title, status)`.
    #[serde(default)]
    pub todos: Vec<(String, String)>,
    /// The goal as the model last left it. Kept as it was written; this app does not read into it.
    #[serde(default)]
    pub goal: Option<Value>,
}

#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

fn dir() -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["agent", "long-tasks"])
}

/// A key as a file name: what could climb out of the folder is replaced. A key that had to change gets a short
/// hash of the original, so two different keys never share a file.
fn file_stem(key: &str) -> String {
    let clean: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if clean == key {
        return clean;
    }
    let sum = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{clean}-{sum:016x}")
}

fn path_in(root: &Path, key: &str) -> PathBuf {
    root.join(format!("{}.json", file_stem(key)))
}

static LOCK: Mutex<()> = Mutex::new(());

fn read(path: &Path) -> Option<Ledger> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Reads the ledger of `key`, lets `f` change it, writes it back. `None` when there is no ledger.
fn update_in<R>(root: &Path, key: &str, f: impl FnOnce(&mut Ledger) -> R) -> Option<R> {
    let _one_at_a_time = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let path = path_in(root, key);
    let mut ledger = read(&path)?;
    let result = f(&mut ledger);
    ledger.updated_at = now_ms();
    // A ledger that cannot be written is one that did not change; the next look reads what is there.
    let _ = vivianne_ai::paths::write_json_atomic(&path, &ledger);
    Some(result)
}

fn write_in(root: &Path, ledger: &Ledger) {
    let _one_at_a_time = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = vivianne_ai::paths::write_json_atomic(&path_in(root, &ledger.key), ledger);
}

/// A new task for `thread`, replacing what the last one left. `cap_usd` and `cap_hours` are the limits it may not
/// pass; none means it runs until it is done.
#[must_use]
pub fn begin(
    thread: &str,
    objective: &str,
    workspace: Option<&str>,
    cap_usd: Option<f64>,
    cap_hours: Option<f64>,
) -> Ledger {
    begin_in(&dir(), thread, objective, workspace, cap_usd, cap_hours)
}

fn begin_in(
    root: &Path,
    thread: &str,
    objective: &str,
    workspace: Option<&str>,
    cap_usd: Option<f64>,
    cap_hours: Option<f64>,
) -> Ledger {
    let now = now_ms();
    let ledger = Ledger {
        key: thread.to_owned(),
        thread: thread.to_owned(),
        workspace: workspace.map(str::to_owned),
        objective: objective.trim().chars().take(300).collect(),
        status: Status::Running,
        cap_usd: cap_usd.filter(|v| *v > 0.0),
        // Hours as milliseconds; a cap is a handful of hours, nowhere near the precision limit.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        deadline_ms: cap_hours
            .filter(|h| *h > 0.0)
            .map(|h| now + (h * 3_600_000.0) as u64),
        started_at: now,
        updated_at: now,
        spent_usd: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        steps: 0,
        model: None,
        wait_until: None,
        status_reason: None,
        milestones: Vec::new(),
        todos: Vec::new(),
        goal: None,
    };
    write_in(root, &ledger);
    ledger
}

/// The ledger of `thread`.
#[must_use]
pub fn get(thread: &str) -> Option<Ledger> {
    read(&path_in(&dir(), thread))
}

/// Takes the task up again under the same ledger: its spend and checkpoints stay.
#[must_use]
pub fn resume(thread: &str) -> Option<Ledger> {
    update_in(&dir(), thread, |l| {
        l.set_status(Status::Running, None);
        l.clone()
    })
}

/// Adds what one model call used.
pub fn record_usage(thread: &str, model: &str, input: u64, output: u64, cost: Option<f64>) {
    update_in(&dir(), thread, |l| {
        l.input_tokens += input;
        l.output_tokens += output;
        // A model with no known price adds nothing: unknown is not zero, but the total must not be made up.
        l.spent_usd += cost.unwrap_or(0.0);
        l.model = Some(model.to_owned());
    });
}

/// One more tool call done.
pub fn record_step(thread: &str) {
    update_in(&dir(), thread, |l| l.steps += 1);
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Stopped,
    /// Stopped on its own and goes on when asked: why.
    Paused(String),
    /// Goes on by itself at the time (epoch ms): why.
    Waiting(u64, String),
}

/// Notes how a run ended. A pause the user asked for during the run wins over how the run itself ended.
pub fn finish(thread: &str, outcome: Outcome) {
    let asked_to_pause = take_pause_request(thread);
    update_in(&dir(), thread, |l| {
        let (status, reason) = match outcome {
            Outcome::Done if asked_to_pause => (Status::Paused, Some("paused by you".to_owned())),
            Outcome::Done => (Status::Done, None),
            Outcome::Stopped => (Status::Stopped, None),
            Outcome::Paused(why) => (Status::Paused, Some(why)),
            Outcome::Waiting(until, why) => {
                l.wait_until = Some(until);
                (Status::Waiting, Some(why))
            }
        };
        l.set_status(status, reason);
    });
}

/// Threads the user asked to pause rather than stop. Kept in memory as well as on disk: the run notes its own
/// ending from its own copy, which could land between the request and the run noticing it.
static PAUSE_REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The user asks for the task of `thread` to pause once the reply being written is done.
pub fn request_pause(thread: &str) {
    PAUSE_REQUESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(thread.to_owned());
}

/// Whether a pause was asked for; clears the request.
pub fn take_pause_request(thread: &str) -> bool {
    let mut asked = PAUSE_REQUESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let found = asked.iter().any(|t| t == thread);
    asked.retain(|t| t != thread);
    found
}

/// Raises the caps so a paused task can go on. Adding to a cap already passed starts from what was spent.
pub fn extend(thread: &str, add_usd: Option<f64>, add_hours: Option<f64>) {
    update_in(&dir(), thread, |l| l.extend(add_usd, add_hours));
}

/// Removes a thread's ledger, with the thread.
pub fn forget(thread: &str) {
    let _one_at_a_time = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = std::fs::remove_file(path_in(&dir(), thread));
}

/// Tasks a relaunched app should offer to take up in `workspace`: still running or waiting when the app closed,
/// touched in the last week, and with a saved thread to carry on in. (What the old app left under other keys has
/// no such thread and is not offered.)
#[must_use]
pub fn resumable(workspace: &str, has_thread: impl Fn(&str) -> bool) -> Vec<Ledger> {
    resumable_in(&dir(), workspace, now_ms(), has_thread)
}

const WEEK_MS: u64 = 7 * 24 * 3_600_000;

fn resumable_in(
    root: &Path,
    workspace: &str,
    now: u64,
    has_thread: impl Fn(&str) -> bool,
) -> Vec<Ledger> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read(&e.path()))
        .filter(|l| {
            l.status.resumes_on_launch()
                && l.workspace.as_deref() == Some(workspace)
                && now.saturating_sub(l.updated_at) < WEEK_MS
                && has_thread(&l.key)
        })
        .collect()
}

/// Deletes the ledgers of tasks that ended long ago, so the folder does not fill up.
pub fn prune(max_age_days: u64) {
    prune_in(&dir(), now_ms(), max_age_days);
}

fn prune_in(root: &Path, now: u64, max_age_days: u64) {
    let _one_at_a_time = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(l) = read(&path)
            && l.status.is_over()
            && now.saturating_sub(l.updated_at) > max_age_days * 24 * 3_600_000
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Ledger {
    pub fn set_status(&mut self, status: Status, reason: Option<String>) {
        self.status = status;
        self.status_reason = reason;
        if status != Status::Waiting {
            self.wait_until = None;
        }
    }

    /// A reason to stop spending, when a cap is reached.
    #[must_use]
    pub fn cap_reached(&self, now: u64) -> Option<String> {
        if let Some(cap) = self.cap_usd
            && self.spent_usd >= cap
        {
            return Some(format!(
                "reached its ${cap:.2} budget (spent ${:.2})",
                self.spent_usd
            ));
        }
        if self.deadline_ms.is_some_and(|deadline| now >= deadline) {
            return Some("reached its time limit".to_owned());
        }
        None
    }

    fn extend(&mut self, add_usd: Option<f64>, add_hours: Option<f64>) {
        if let Some(add) = add_usd.filter(|v| *v > 0.0) {
            self.cap_usd = Some(self.cap_usd.unwrap_or(self.spent_usd).max(self.spent_usd) + add);
        }
        if let Some(hours) = add_hours.filter(|v| *v > 0.0) {
            let base = self.deadline_ms.unwrap_or(0).max(now_ms());
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let more = (hours * 3_600_000.0) as u64;
            self.deadline_ms = Some(base + more);
        }
    }

    /// Takes the plan as the agent last wrote it, turning each item that has newly become `completed` into a
    /// milestone. Returns the new ones.
    pub fn record_todos(&mut self, todos: &[(String, String, Option<String>)]) -> Vec<Milestone> {
        let now = now_ms();
        let mut added = Vec::new();
        for (title, status, summary) in todos {
            if status != "completed" || self.milestones.iter().any(|m| &m.title == title) {
                continue;
            }
            let since = self.milestones.last().map_or(self.started_at, |m| m.at);
            let spent_before: f64 = self.milestones.iter().map(|m| m.spent_usd).sum();
            let milestone = Milestone {
                title: title.clone(),
                summary: summary.clone().filter(|s| !s.trim().is_empty()),
                at: now,
                spent_usd: (self.spent_usd - spent_before).max(0.0),
                elapsed_ms: now.saturating_sub(since),
            };
            self.milestones.push(milestone.clone());
            added.push(milestone);
        }
        self.todos = todos
            .iter()
            .map(|(t, s, _)| (t.clone(), s.clone()))
            .collect();
        added
    }

    /// Where the run stands, written so the model can pick up from it: given to it on resume and after compaction,
    /// when its own memory of the run is gone.
    #[must_use]
    pub fn briefing(&self) -> String {
        use std::fmt::Write as _;
        let mut out = format!("<long_task>\nObjective: {}\n", self.objective);
        if self.milestones.is_empty() {
            out.push_str("Completed so far: nothing yet.\n");
        } else {
            out.push_str("Completed so far:\n");
            for (i, m) in self.milestones.iter().enumerate() {
                let _ = match &m.summary {
                    Some(s) => writeln!(out, "  {}. {} — {s}", i + 1, m.title),
                    None => writeln!(out, "  {}. {}", i + 1, m.title),
                };
            }
        }
        let ahead: Vec<_> = self
            .todos
            .iter()
            .filter(|(_, s)| s != "completed")
            .collect();
        if !ahead.is_empty() {
            out.push_str("Still to do:\n");
            for (title, status) in ahead {
                let _ = writeln!(
                    out,
                    "  - {title}{}",
                    if status == "in_progress" {
                        " (in progress)"
                    } else {
                        ""
                    }
                );
            }
        }
        let mut spend = format!("Spent ${:.2}", self.spent_usd);
        if let Some(cap) = self.cap_usd {
            let _ = write!(spend, " of ${cap:.2}");
        }
        let _ = write!(out, "{spend}, {} steps so far.\n</long_task>", self.steps);
        out
    }

    /// One line for the thread: what has been spent and done.
    #[must_use]
    pub fn line(&self) -> String {
        let mut line = format!("${:.2}", self.spent_usd);
        if let Some(cap) = self.cap_usd {
            let _ = std::fmt::Write::write_fmt(&mut line, format_args!(" of ${cap:.2}"));
        }
        format!("{line} · {} steps", self.steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-ledger-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn two_different_keys_never_share_a_file_and_none_climbs_out() {
        assert_eq!(file_stem("a1-b_2"), "a1-b_2");
        assert_ne!(file_stem("a/b"), file_stem("a-b"));
        assert!(!file_stem("../../etc/passwd").contains(['/', '\\', '.']));
    }

    #[test]
    fn a_task_counts_what_it_spends_and_is_stopped_by_its_caps() {
        let root = scratch();
        let mut l = begin_in(&root, "t1", "fix the parser", Some("/w"), Some(1.0), None);
        assert_eq!(l.cap_reached(now_ms()), None);
        update_in(&root, "t1", |l| {
            l.spent_usd += 1.5;
            l.steps += 3;
        });
        l = read(&path_in(&root, "t1")).expect("saved");
        assert!(
            l.cap_reached(now_ms())
                .is_some_and(|why| why.contains("$1.00"))
        );
        let mut raised = l.clone();
        raised.extend(Some(2.0), None);
        assert_eq!(
            raised.cap_usd,
            Some(3.5),
            "a cap already passed grows from what was spent"
        );
        let timed = Ledger {
            cap_usd: None,
            deadline_ms: Some(5),
            ..l
        };
        assert!(timed.cap_reached(6).is_some_and(|why| why.contains("time")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_pause_the_user_asked_for_beats_a_run_that_ended_fine() {
        let root = scratch();
        begin_in(&root, "t-pause", "x", None, None, None);
        request_pause("t-pause");
        assert!(take_pause_request("t-pause"));
        assert!(!take_pause_request("t-pause"), "a request is taken once");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_work_that_was_cut_off_in_this_project_and_can_be_carried_on_is_offered() {
        let root = scratch();
        begin_in(&root, "running", "a", Some("/w"), None, None);
        begin_in(&root, "elsewhere", "b", Some("/other"), None, None);
        begin_in(&root, "no-thread", "c", Some("/w"), None, None);
        begin_in(&root, "finished", "d", Some("/w"), None, None);
        update_in(&root, "finished", |l| l.set_status(Status::Done, None));
        let offered: Vec<String> = resumable_in(&root, "/w", now_ms(), |key| key != "no-thread")
            .into_iter()
            .map(|l| l.key)
            .collect();
        assert_eq!(offered, ["running"]);
        assert!(
            resumable_in(&root, "/w", now_ms() + 8 * 24 * 3_600_000, |_| true).is_empty(),
            "a week old is let go"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn old_tasks_that_ended_are_cleaned_up_and_live_ones_stay() {
        let root = scratch();
        begin_in(&root, "done-old", "a", None, None, None);
        update_in(&root, "done-old", |l| l.set_status(Status::Done, None));
        begin_in(&root, "live", "b", None, None, None);
        prune_in(&root, now_ms() + 40 * 24 * 3_600_000, 30);
        assert!(read(&path_in(&root, "done-old")).is_none());
        assert!(read(&path_in(&root, "live")).is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn completed_plan_items_become_milestones_once_and_the_briefing_says_what_is_left() {
        let mut l = begin_in(&scratch(), "t2", "ship it", None, Some(5.0), None);
        l.spent_usd = 0.4;
        let plan = vec![
            (
                "Write parser".to_owned(),
                "completed".to_owned(),
                Some("done".to_owned()),
            ),
            ("Add tests".to_owned(), "in_progress".to_owned(), None),
        ];
        assert_eq!(l.record_todos(&plan).len(), 1);
        assert!(
            l.record_todos(&plan).is_empty(),
            "a milestone is recorded once"
        );
        let brief = l.briefing();
        assert!(
            brief.contains("1. Write parser — done")
                && brief.contains("Add tests (in progress)")
                && brief.contains("of $5.00"),
            "{brief}"
        );
    }

    #[test]
    fn the_old_apps_ledger_files_still_load() {
        let json = r#"{"key":"k","objective":"o","status":"waiting","startedAt":1,"updatedAt":2,"waitUntil":9,"milestones":[{"title":"t","at":3}],"goal":{"phase":"x"}}"#;
        let l: Ledger = serde_json::from_str(json).expect("loads");
        assert_eq!(
            (l.status, l.wait_until, l.milestones.len()),
            (Status::Waiting, Some(9), 1)
        );
    }
}
