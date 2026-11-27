//! Tasks kept in `~/.viv/agent/schedules.json`, changed under an OS file lock so two Viviannes cannot lose each
//! other's edits. A due task is first looked at and only marked delivered once it has been sent, so a busy
//! conversation keeps it waiting instead of dropping it. A file from a newer build is not rewritten.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::domain::{self, ScheduleError, Target, Task, Timing, is_recurring};

const VERSION: u32 = 1;
/// Deliveries remembered per task.
pub const HISTORY: usize = 20;
pub const MAX_TASKS: usize = 200;
pub const MAX_PAGE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("there is no scheduled task {0}")]
    NotFound(String),
    #[error("the task changed since it was read; view it again")]
    Conflict,
    #[error("there are already {MAX_TASKS} tasks; delete some first")]
    Full,
    #[error("{0}")]
    Invalid(#[from] ScheduleError),
    #[error("schedules.json is from a newer Vivianne (format {0}) and was not changed")]
    Newer(u32),
    #[error("schedules.json cannot be read: {0}")]
    Damaged(String),
    #[error("{0}")]
    Io(String),
}

/// One delivery that happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub occurrence_at: i64,
    pub delivered_at: i64,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub task: Task,
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    tasks: Vec<Entry>,
}

/// A task that is due, as read; hand it back to [`Store::delivered`] once it has been sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub entry: Entry,
    pub occurrence_at: i64,
    pub next_scheduled_at: Option<i64>,
}

/// Changes to a task; what is absent stays.
#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub timing: Option<Timing>,
    pub target: Option<Target>,
    pub active: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    #[must_use]
    pub fn at(path: &Path) -> Self {
        Self { path: path.to_owned() }
    }

    #[must_use]
    pub fn standard() -> Self {
        Self::at(&vivianne_ai::paths::viv_home_path(&["agent", "schedules.json"]))
    }

    fn read(&self) -> Result<File, StoreError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(File { version: VERSION, tasks: Vec::new() });
            }
            Err(e) => return Err(StoreError::Io(e.to_string())),
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| StoreError::Damaged(e.to_string()))?;
        let version = value["version"].as_u64().and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
        if version > VERSION {
            return Err(StoreError::Newer(version));
        }
        serde_json::from_value(value).map_err(|e| StoreError::Damaged(e.to_string()))
    }

    /// Runs `change` on the file while holding the lock, and writes it back if the change succeeds.
    fn change<T>(&self, change: impl FnOnce(&mut Vec<Entry>) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let io = |e: std::io::Error| StoreError::Io(e.to_string());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path.with_extension("lock"))
            .map_err(io)?;
        lock.lock().map_err(io)?;
        let mut file = self.read()?;
        let out = change(&mut file.tasks)?;
        file.version = VERSION;
        vivianne_ai::paths::write_json_atomic(&self.path, &file).map_err(StoreError::Io)?;
        Ok(out)
    }

    pub fn add(&self, task: Task) -> Result<Entry, StoreError> {
        self.change(|tasks| {
            if tasks.len() >= MAX_TASKS {
                return Err(StoreError::Full);
            }
            let entry = Entry { task, deliveries: Vec::new() };
            tasks.push(entry.clone());
            Ok(entry)
        })
    }

    pub fn view(&self, id: &str) -> Result<Entry, StoreError> {
        self.read()?
            .tasks
            .into_iter()
            .find(|e| e.task.id == id)
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// Tasks, soonest due first, with how many match. `active` narrows to running or paused/ended ones.
    pub fn list(&self, active: Option<bool>, offset: usize, limit: usize) -> Result<(Vec<Entry>, usize), StoreError> {
        let mut tasks = self.read()?.tasks;
        tasks.retain(|e| active.is_none_or(|a| e.task.active == a));
        tasks.sort_by(|a, b| (!a.task.active, a.task.scheduled_at, &a.task.id).cmp(&(!b.task.active, b.task.scheduled_at, &b.task.id)));
        let total = tasks.len();
        let page = tasks.into_iter().skip(offset).take(limit.clamp(1, MAX_PAGE)).collect();
        Ok((page, total))
    }

    /// Changes a task. `expected` is the `updated_at` the caller read; a task changed since is left alone.
    pub fn edit(&self, id: &str, patch: &Patch, expected: Option<i64>, now: i64) -> Result<Entry, StoreError> {
        self.change(|tasks| {
            let entry = tasks.iter_mut().find(|e| e.task.id == id).ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
            if expected.is_some_and(|at| at != entry.task.updated_at) {
                return Err(StoreError::Conflict);
            }
            let mut task = entry.task.clone();
            if let Some(title) = &patch.title {
                task.title = domain::check_title(title)?;
            }
            if let Some(prompt) = &patch.prompt {
                task.prompt = domain::check_prompt(prompt)?;
            }
            if let Some(target) = &patch.target {
                task.target = target.clone();
            }
            if let Some(timing) = &patch.timing {
                task = domain::retime(&task, timing, now)?;
            }
            if let Some(active) = patch.active {
                // Taking up a paused task again does not deliver what it missed while paused.
                if active && !task.active && patch.timing.is_none() {
                    task = domain::retime(&task, &task.timing.clone(), now)?;
                }
                task.active = active;
            }
            // Always moves on, so two edits in the same millisecond still conflict.
            task.updated_at = now.max(entry.task.updated_at + 1);
            entry.task = task;
            Ok(entry.clone())
        })
    }

    pub fn delete(&self, id: &str, expected: Option<i64>) -> Result<(), StoreError> {
        self.change(|tasks| {
            let at = tasks.iter().position(|e| e.task.id == id).ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
            if expected.is_some_and(|stamp| stamp != tasks[at].task.updated_at) {
                return Err(StoreError::Conflict);
            }
            tasks.remove(at);
            Ok(())
        })
    }

    /// The active tasks due at `now`, soonest first. Nothing is changed: see [`Store::delivered`].
    pub fn due(&self, now: i64) -> Result<Vec<Due>, StoreError> {
        let mut due: Vec<Due> = self
            .read()?
            .tasks
            .into_iter()
            .filter(|e| e.task.active && e.task.scheduled_at <= now)
            .filter_map(|entry| {
                let (occurrence_at, next_scheduled_at) = if is_recurring(&entry.task.timing) {
                    let found = domain::resolve(&entry.task, now)?;
                    (found.occurrence_at, found.next_scheduled_at)
                } else {
                    (entry.task.scheduled_at, None)
                };
                Some(Due { entry, occurrence_at, next_scheduled_at })
            })
            .collect();
        due.sort_by_key(|d| (d.occurrence_at, d.entry.task.id.clone()));
        Ok(due)
    }

    /// Records that `due` was sent to `session_id` and moves the task on. A task edited, delivered or removed since it
    /// was read is left as it is (`Conflict`).
    pub fn delivered(&self, due: &Due, session_id: &str, now: i64) -> Result<(), StoreError> {
        self.change(|tasks| {
            let entry = tasks
                .iter_mut()
                .find(|e| e.task.id == due.entry.task.id)
                .ok_or(StoreError::Conflict)?;
            if entry.task.updated_at != due.entry.task.updated_at || entry.task.scheduled_at != due.entry.task.scheduled_at {
                return Err(StoreError::Conflict);
            }
            match due.next_scheduled_at {
                Some(next) => entry.task.scheduled_at = next,
                None => entry.task.active = false,
            }
            // A conversation made once is now the one it reuses.
            if let Target::NewOnce { session_id: made, .. } = &mut entry.task.target {
                session_id.clone_into(made);
            }
            entry.deliveries.push(Delivery { occurrence_at: due.occurrence_at, delivered_at: now, session_id: session_id.to_owned() });
            let excess = entry.deliveries.len().saturating_sub(HISTORY);
            entry.deliveries.drain(..excess);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::domain::CreateSpec;

    const MIN: i64 = 60_000;
    const NOW: i64 = 1_800_000_000_000;

    fn store(name: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!("viv-sched-{name}-{}", uuid::Uuid::new_v4().simple()));
        let store = Store::at(&dir.join("schedules.json"));
        (dir, store)
    }

    fn task(id: &str, timing: Timing) -> Task {
        let target = Target::NewOnce { session_id: String::new(), create: CreateSpec::default() };
        domain::create(id, "t", "p", &timing, target, NOW).expect("task")
    }

    fn every() -> Timing {
        Timing::Every { every_seconds: 60 }
    }

    #[test]
    fn tasks_are_listed_soonest_first_edited_under_a_stamp_and_deleted() {
        let (dir, store) = store("crud");
        store.add(task("b", Timing::After { after_seconds: 600 })).expect("added");
        store.add(task("a", Timing::After { after_seconds: 60 })).expect("added");
        let (list, total) = store.list(None, 0, 10).expect("listed");
        assert_eq!((total, list[0].task.id.as_str()), (2, "a"));
        assert_eq!(store.list(Some(false), 0, 10).expect("listed").1, 0);
        let seen = store.view("a").expect("viewed").task.updated_at;
        let patch = Patch { title: Some("new".into()), ..Patch::default() };
        let edited = store.edit("a", &patch, Some(seen), NOW).expect("edited");
        assert_eq!(edited.task.title, "new");
        assert_eq!(store.edit("a", &patch, Some(seen), NOW), Err(StoreError::Conflict));
        let bad = Patch { title: Some(" ".into()), ..Patch::default() };
        assert!(matches!(store.edit("a", &bad, None, NOW), Err(StoreError::Invalid(_))));
        assert_eq!(store.delete("a", Some(seen)), Err(StoreError::Conflict));
        store.delete("a", None).expect("deleted");
        assert_eq!(store.view("a"), Err(StoreError::NotFound("a".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_due_task_waits_until_marked_and_a_miss_is_delivered_once() {
        let (dir, store) = store("due");
        store.add(task("r", every())).expect("added");
        store.add(task("o", Timing::After { after_seconds: 30 })).expect("added");
        assert!(store.due(NOW).expect("due").is_empty());
        let later = NOW + 10 * MIN + 5;
        let due = store.due(later).expect("due");
        assert_eq!(due.len(), 2);
        assert_eq!(store.due(later).expect("due"), due, "looking changes nothing");
        let recurring = due.iter().find(|d| d.entry.task.id == "r").expect("r");
        assert_eq!(recurring.occurrence_at, NOW + 10 * MIN, "the latest missed one only");
        for d in &due {
            store.delivered(d, "s1", later).expect("delivered");
        }
        assert!(store.due(later).expect("due").is_empty());
        let r = store.view("r").expect("r");
        assert_eq!((r.task.active, r.task.scheduled_at), (true, NOW + 11 * MIN));
        assert_eq!(r.deliveries.len(), 1);
        assert!(matches!(&r.task.target, Target::NewOnce { session_id, .. } if session_id == "s1"));
        assert!(!store.view("o").expect("o").task.active, "a one-shot ends");
        assert_eq!(store.delivered(recurring, "s2", later), Err(StoreError::Conflict), "not twice");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resuming_a_paused_task_does_not_send_the_backlog() {
        let (dir, store) = store("pause");
        store.add(task("r", every())).expect("added");
        let off = Patch { active: Some(false), ..Patch::default() };
        store.edit("r", &off, None, NOW).expect("paused");
        assert!(store.due(NOW + 100 * MIN).expect("due").is_empty());
        let on = Patch { active: Some(true), ..Patch::default() };
        store.edit("r", &on, None, NOW + 100 * MIN).expect("resumed");
        assert!(store.due(NOW + 100 * MIN).expect("due").is_empty());
        assert_eq!(store.view("r").expect("r").task.scheduled_at, NOW + 101 * MIN);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_is_capped_and_a_newer_or_damaged_file_is_left_alone() {
        let (dir, store) = store("files");
        store.add(task("r", every())).expect("added");
        for i in 1..=i64::try_from(HISTORY).expect("small") + 5 {
            let due = store.due(NOW + i * MIN).expect("due");
            store.delivered(&due[0], "s", NOW + i * MIN).expect("delivered");
        }
        assert_eq!(store.view("r").expect("r").deliveries.len(), HISTORY);
        let path = dir.join("schedules.json");
        for (text, want) in [(r#"{"version":9,"tasks":[]}"#, StoreError::Newer(9))] {
            std::fs::write(&path, text).expect("written");
            assert_eq!(store.add(task("x", every())), Err(want));
            assert_eq!(std::fs::read_to_string(&path).expect("kept"), text);
        }
        std::fs::write(&path, "{ not json").expect("written");
        assert!(matches!(store.add(task("x", every())), Err(StoreError::Damaged(_))));
        assert_eq!(std::fs::read_to_string(&path).expect("kept"), "{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
