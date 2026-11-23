//! Threads kept on disk, one JSON file each under `~/.viv/agent/threads/`, so a conversation survives closing the
//! app. A thread belongs to the project folder it was started in.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use vivianne_core::{Message, Mode};

/// One conversation as saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedThread {
    pub id: String,
    /// The project folder it was started in.
    pub workspace: String,
    pub title: String,
    /// `user` when the person named it, `auto` otherwise (see [`crate::title`]).
    pub title_source: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub mode: Mode,
    /// Seconds since 1970.
    pub updated_at: u64,
    pub history: Vec<Message>,
}

/// A thread as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
}

#[must_use]
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// An id is a file name, so only the characters a uuid has are allowed: it can never name another folder.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn default_dir() -> Result<PathBuf, String> {
    Ok(vivianne_ai::paths::viv_agent_dir()?.join("threads"))
}

/// Saves `thread`, replacing an earlier save of it.
pub fn save(thread: &SavedThread) -> Result<(), String> {
    save_in(&default_dir()?, thread)
}

/// The threads started in `workspace`, newest first. Files that cannot be read are skipped, never fatal.
#[must_use]
pub fn list(workspace: &str) -> Vec<Summary> {
    default_dir()
        .map(|dir| list_in(&dir, workspace))
        .unwrap_or_default()
}

#[must_use]
pub fn load(id: &str) -> Option<SavedThread> {
    load_in(&default_dir().ok()?, id)
}

pub fn delete(id: &str) -> Result<(), String> {
    delete_in(&default_dir()?, id)
}

fn save_in(dir: &Path, thread: &SavedThread) -> Result<(), String> {
    if !valid_id(&thread.id) {
        return Err(format!("not a thread id: {}", thread.id));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // What the desktop showed is not kept.
    let kept = SavedThread {
        history: crate::history::without_desktop_content(&thread.history),
        ..thread.clone()
    };
    vivianne_ai::paths::write_json_atomic(&dir.join(format!("{}.json", thread.id)), &kept)
}

fn load_in(dir: &Path, id: &str) -> Option<SavedThread> {
    if !valid_id(id) {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?).ok()
}

/// Every saved thread, with the project folder it was started in, newest first. For a remote client that does not
/// know which projects this machine has.
#[must_use]
pub fn list_everywhere() -> Vec<(String, Summary)> {
    let Ok(dir) = default_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, Summary)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| load_in(&dir, e.path().file_stem()?.to_str()?))
        .filter(|t| !t.history.is_empty())
        .map(|t| {
            (
                t.workspace,
                Summary {
                    id: t.id,
                    title: t.title,
                    updated_at: t.updated_at,
                },
            )
        })
        .collect();
    found.sort_by(|a, b| {
        b.1.updated_at
            .cmp(&a.1.updated_at)
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    found
}

fn list_in(dir: &Path, workspace: &str) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Summary> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| load_in(dir, e.path().file_stem()?.to_str()?))
        .filter(|t| t.workspace == workspace && !t.history.is_empty())
        .map(|t| Summary {
            id: t.id,
            title: t.title,
            updated_at: t.updated_at,
        })
        .collect();
    found.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    found
}

fn delete_in(dir: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("not a thread id: {id}"));
    }
    match std::fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use vivianne_core::Role;

    use super::*;

    fn thread(id: &str, workspace: &str, updated_at: u64) -> SavedThread {
        SavedThread {
            id: id.into(),
            workspace: workspace.into(),
            title: format!("title {id}"),
            title_source: "auto".into(),
            provider: "p".into(),
            model: "m".into(),
            mode: Mode::Ask,
            updated_at,
            history: vec![Message::new(Role::User, "hi")],
        }
    }

    #[test]
    fn threads_round_trip_are_listed_newest_first_per_project_and_ids_cannot_escape() {
        let dir = std::env::temp_dir().join(format!("viv-threads-{}", uuid::Uuid::new_v4()));
        save_in(&dir, &thread("a", "/p1", 10)).unwrap();
        save_in(&dir, &thread("b", "/p1", 20)).unwrap();
        save_in(&dir, &thread("c", "/p2", 30)).unwrap();
        let mut empty = thread("d", "/p1", 40);
        empty.history.clear();
        save_in(&dir, &empty).unwrap();
        std::fs::write(dir.join("broken.json"), "{not json").unwrap();

        let ids: Vec<_> = list_in(&dir, "/p1").into_iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            ["b", "a"],
            "newest first; other projects, empty threads and unreadable files left out"
        );
        assert_eq!(load_in(&dir, "a"), Some(thread("a", "/p1", 10)));

        assert!(save_in(&dir, &thread("../evil", "/p1", 1)).is_err());
        assert!(load_in(&dir, "../a").is_none() && delete_in(&dir, "../a").is_err());
        delete_in(&dir, "a").unwrap();
        delete_in(&dir, "a").unwrap();
        assert!(load_in(&dir, "a").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
