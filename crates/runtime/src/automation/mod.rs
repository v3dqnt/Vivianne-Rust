//! Automations: prompts that run by themselves, on a cron schedule or when a file appears in a folder. They live in
//! `~/.viv/agent/automations.json`, the old app's file, so the ones already there keep working.
//!
//! This module only decides *what is due*: [`due`] looks at the list, moves each fired automation's clock forward
//! and says which fired. Running one is the app's job (it starts a thread with the prompt), because a timer must
//! not drive an agent that may be in the middle of a conversation.

mod tools;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Local, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};

pub use tools::{CreateAutomation, DeleteAutomation, ListAutomations, SetAutomationEnabled};

/// One automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    pub name: String,
    /// What is sent to the agent when this fires.
    pub prompt: String,
    /// Standard 5-field cron (`min hour day-of-month month day-of-week`), or six fields with seconds first.
    /// `0 9 * * *` is nine o'clock every day, in the user's own time zone.
    pub cron: String,
    pub enabled: bool,
    pub last_run: Option<u64>,
    pub next_run: Option<u64>,
    pub created_at: u64,
    /// More steps after `prompt`, each run in order in the same conversation.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Fire when a file appears or changes in this folder, instead of on `cron`.
    #[serde(default)]
    pub folder: Option<String>,
    /// Only files whose name matches (`*` and `?`); none means every file.
    #[serde(default)]
    pub glob: Option<String>,
    /// File name to modified time at the last look. `None` until the folder has been looked at once, so what is
    /// there already does not all fire.
    #[serde(default)]
    pub seen: Option<HashMap<String, u64>>,
}

/// One later step of a workflow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub prompt: String,
    /// Stop and ask the user before running this step.
    #[serde(default)]
    pub ask_first: bool,
}

/// An automation that fired, and the file that set it off when a folder did.
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    pub automation: Automation,
    pub file: Option<String>,
}

#[must_use]
pub fn now_ms() -> u64 {
    u64::try_from(Utc::now().timestamp_millis()).unwrap_or(0)
}

/// `*` (any run) and `?` (one character) against a file name, ignoring case.
#[must_use]
pub fn glob_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a == b && go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    go(&p, &n)
}

/// Which of a folder's files are new or changed since `seen`, updating it. The first look only records. A file
/// modified in the last few seconds waits for the next look, so one still being written is not picked up half done.
pub fn changed_files(
    listing: &[(String, u64)],
    seen: &mut Option<HashMap<String, u64>>,
    glob: Option<&str>,
    now: u64,
) -> Vec<String> {
    const SETTLE_MS: u64 = 3_000;
    let glob = glob.map(str::trim).filter(|g| !g.is_empty());
    let first = seen.is_none();
    let map = seen.get_or_insert_with(HashMap::new);
    let mut out = Vec::new();
    for (name, mtime) in listing {
        if glob.is_some_and(|g| !glob_match(g, name)) || map.get(name) == Some(mtime) {
            continue;
        }
        if !first && now.saturating_sub(*mtime) < SETTLE_MS {
            continue;
        }
        map.insert(name.clone(), *mtime);
        if !first {
            out.push(name.clone());
        }
    }
    out
}

/// A folder's files (not its subfolders) with their modified times in milliseconds.
fn list_folder(dir: &str) -> Vec<(String, u64)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some((
                entry.file_name().to_string_lossy().into_owned(),
                u64::try_from(modified.as_millis()).ok()?,
            ))
        })
        .collect()
}

/// The cron crate wants six fields (seconds first); the familiar five get a `0` seconds column.
fn schedule(expression: &str) -> Option<Schedule> {
    let text = if expression.split_whitespace().count() == 5 {
        format!("0 {}", expression.trim())
    } else {
        expression.trim().to_owned()
    };
    Schedule::from_str(&text).ok()
}

#[must_use]
pub fn is_valid_cron(expression: &str) -> bool {
    schedule(expression).is_some()
}

/// The next time (epoch ms) strictly after `after` that `expression` fires, or `None` for a bad expression. The
/// fields mean the user's local time ("0 11 * * *" is 11 in the morning where they are), not UTC.
#[must_use]
pub fn next_run_after(expression: &str, after: DateTime<Utc>) -> Option<u64> {
    let next = schedule(expression)?
        .after(&after.with_timezone(&Local))
        .next()?;
    u64::try_from(next.with_timezone(&Utc).timestamp_millis()).ok()
}

// ── Storage ───────────────────────────────────────────────────────────────────────────────────────────────────

fn file() -> std::path::PathBuf {
    vivianne_ai::paths::viv_home_path(&["agent", "automations.json"])
}

fn read(path: &std::path::Path) -> Vec<Automation> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// One writer at a time: the list is read, changed and written back under this.
static LOCK: Mutex<()> = Mutex::new(());

fn change<R>(path: &std::path::Path, f: impl FnOnce(&mut Vec<Automation>) -> R) -> R {
    let _one_at_a_time = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut items = read(path);
    let result = f(&mut items);
    // A list that cannot be written is the same as one that was not changed; the next look reads what is there.
    let _ = vivianne_ai::paths::write_json_atomic(path, &items);
    result
}

/// Every automation.
#[must_use]
pub fn list() -> Vec<Automation> {
    read(&file())
}

/// What a new automation is made of.
#[derive(Debug, Clone, Default)]
pub struct New {
    pub name: String,
    pub prompt: String,
    pub cron: String,
    pub steps: Vec<Step>,
    pub folder: Option<String>,
    pub glob: Option<String>,
}

fn clean(steps: Vec<Step>) -> Vec<Step> {
    steps
        .into_iter()
        .map(|s| Step {
            prompt: s.prompt.trim().to_owned(),
            ask_first: s.ask_first,
        })
        .filter(|s| !s.prompt.is_empty())
        .collect()
}

/// Makes an automation, enabled.
///
/// # Errors
/// A missing name or prompt, a bad cron expression, or a folder that is not one.
pub fn create(new: New) -> Result<Automation, String> {
    create_in(&file(), new)
}

fn create_in(path: &std::path::Path, new: New) -> Result<Automation, String> {
    if new.name.trim().is_empty() || new.prompt.trim().is_empty() {
        return Err("name and prompt are required".to_owned());
    }
    let folder = new
        .folder
        .map(|f| f.trim().to_owned())
        .filter(|f| !f.is_empty());
    match &folder {
        Some(f) if !std::path::Path::new(f).is_dir() => return Err(format!("{f} isn't a folder")),
        None if !is_valid_cron(&new.cron) => {
            return Err(format!("invalid cron expression: {}", new.cron));
        }
        _ => {}
    }
    let automation = Automation {
        id: uuid::Uuid::new_v4().to_string(),
        name: new.name.trim().to_owned(),
        prompt: new.prompt.trim().to_owned(),
        cron: new.cron.trim().to_owned(),
        enabled: true,
        last_run: None,
        next_run: if folder.is_some() {
            None
        } else {
            next_run_after(new.cron.trim(), Utc::now())
        },
        created_at: now_ms(),
        steps: clean(new.steps),
        folder,
        glob: new
            .glob
            .map(|g| g.trim().to_owned())
            .filter(|g| !g.is_empty()),
        seen: None,
    };
    change(path, |items| items.push(automation.clone()));
    Ok(automation)
}

/// Turns one on or off. Turning one on starts its clock from now, so one that was off for a week does not fire for
/// every schedule it slept through.
///
/// # Errors
/// There is no such automation.
pub fn set_enabled(id: &str, enabled: bool) -> Result<Automation, String> {
    change(&file(), |items| {
        let a = items
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("automation not found")?;
        a.enabled = enabled;
        a.next_run = if enabled && a.folder.is_none() {
            next_run_after(&a.cron, Utc::now())
        } else {
            None
        };
        if enabled {
            a.seen = None;
        }
        Ok(a.clone())
    })
}

/// Removes one. False when there was none.
#[must_use]
pub fn delete(id: &str) -> bool {
    change(&file(), |items| {
        let before = items.len();
        items.retain(|a| a.id != id);
        items.len() != before
    })
}

/// Marks one as run now and returns it, for "run now" regardless of its schedule.
///
/// # Errors
/// There is no such automation.
pub fn run_now(id: &str) -> Result<Fired, String> {
    change(&file(), |items| {
        let a = items
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("automation not found")?;
        a.last_run = Some(now_ms());
        Ok(Fired {
            automation: a.clone(),
            file: None,
        })
    })
}

/// What is due at `now`: each enabled automation whose time has come, or whose folder has a new file (up to five a
/// look). Their clocks move on, so the same one is not due again.
#[must_use]
pub fn due(now: u64) -> Vec<Fired> {
    due_in(&file(), now)
}

fn due_in(path: &std::path::Path, now: u64) -> Vec<Fired> {
    change(path, |items| {
        let mut fired = Vec::new();
        for a in items.iter_mut().filter(|a| a.enabled) {
            if let Some(dir) = a.folder.clone() {
                let glob = a.glob.clone();
                for name in changed_files(&list_folder(&dir), &mut a.seen, glob.as_deref(), now)
                    .into_iter()
                    .take(5)
                {
                    a.last_run = Some(now);
                    fired.push(Fired {
                        automation: a.clone(),
                        file: Some(std::path::Path::new(&dir).join(&name).display().to_string()),
                    });
                }
            } else if a.next_run.is_none() {
                // Never had a time worked out: start its clock.
                a.next_run = next_run_after(&a.cron, Utc::now());
            } else if a.next_run.is_some_and(|next| now >= next) {
                a.last_run = Some(now);
                a.next_run = next_run_after(&a.cron, Utc::now());
                fired.push(Fired {
                    automation: a.clone(),
                    file: None,
                });
            }
        }
        fired
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("viv-automations-{}.json", uuid::Uuid::new_v4()))
    }

    #[test]
    fn five_and_six_field_cron_is_accepted_and_garbage_is_not() {
        for ok in ["0 9 * * *", "*/15 * * * *", "0 0 9 * * *"] {
            assert!(is_valid_cron(ok), "{ok}");
        }
        for bad in ["not a cron", "99 99 99 99 99", ""] {
            assert!(!is_valid_cron(bad), "{bad}");
        }
    }

    #[test]
    fn a_glob_matches_names_ignoring_case() {
        assert!(
            glob_match("*.pdf", "Report.PDF")
                && glob_match("scan-??.png", "scan-07.png")
                && glob_match("*", "anything")
        );
        assert!(!glob_match("*.pdf", "report.pdf.txt"));
    }

    #[test]
    fn a_folder_fires_once_per_new_or_changed_file() {
        let now = 1_000_000;
        let mut seen = None;
        let old = vec![("a.txt".to_owned(), 10_u64)];
        assert!(
            changed_files(&old, &mut seen, None, now).is_empty(),
            "the first look only records"
        );
        assert!(changed_files(&old, &mut seen, None, now).is_empty());
        let with_new = vec![("a.txt".to_owned(), 10), ("b.txt".to_owned(), 500_000)];
        assert_eq!(changed_files(&with_new, &mut seen, None, now), ["b.txt"]);
        assert!(changed_files(&with_new, &mut seen, None, now).is_empty());
        let changed = vec![("a.txt".to_owned(), 600_000), ("c.log".to_owned(), 600_000)];
        assert_eq!(
            changed_files(&changed, &mut seen, Some("*.txt"), now),
            ["a.txt"]
        );
        let fresh = vec![("d.txt".to_owned(), now - 500)];
        assert!(
            changed_files(&fresh, &mut seen, None, now).is_empty(),
            "still being written"
        );
        assert_eq!(
            changed_files(&fresh, &mut seen, None, now + 5_000),
            ["d.txt"]
        );
    }

    #[test]
    fn the_old_apps_automations_still_load() {
        let json = r#"{"id":"1","name":"n","prompt":"p","cron":"0 9 * * *","enabled":true,"lastRun":null,"nextRun":5,"createdAt":1}"#;
        let a: Automation = serde_json::from_str(json).expect("loads");
        assert!(a.steps.is_empty() && a.folder.is_none() && a.seen.is_none());
    }

    #[test]
    fn the_next_run_is_in_the_future() {
        let now = Utc::now();
        assert!(
            next_run_after("*/5 * * * *", now)
                .is_some_and(|next| i64::try_from(next).unwrap_or(0) > now.timestamp_millis())
        );
    }

    #[test]
    fn an_automation_fires_when_its_time_comes_and_not_again() {
        let file = scratch();
        let made = create_in(
            &file,
            New {
                name: "n".into(),
                prompt: "p".into(),
                cron: "*/5 * * * *".into(),
                ..New::default()
            },
        )
        .expect("creates");
        assert!(due_in(&file, now_ms()).is_empty(), "not yet");
        // Its time arrives.
        change(&file, |items| items[0].next_run = Some(1));
        let fired = due_in(&file, now_ms());
        assert_eq!(
            fired
                .iter()
                .map(|f| f.automation.id.as_str())
                .collect::<Vec<_>>(),
            [made.id.as_str()]
        );
        assert!(due_in(&file, now_ms()).is_empty(), "its clock moved on");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_bad_automation_is_refused() {
        let file = scratch();
        assert!(
            create_in(
                &file,
                New {
                    name: "".into(),
                    prompt: "p".into(),
                    cron: "0 9 * * *".into(),
                    ..New::default()
                }
            )
            .is_err()
        );
        assert!(
            create_in(
                &file,
                New {
                    name: "n".into(),
                    prompt: "p".into(),
                    cron: "nope".into(),
                    ..New::default()
                }
            )
            .is_err()
        );
        assert!(
            create_in(
                &file,
                New {
                    name: "n".into(),
                    prompt: "p".into(),
                    folder: Some("/no/such/folder".into()),
                    ..New::default()
                }
            )
            .is_err()
        );
        assert!(list_in_test(&file).is_empty());
    }

    fn list_in_test(path: &std::path::Path) -> Vec<Automation> {
        read(path)
    }
}
