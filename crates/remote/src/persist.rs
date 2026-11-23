//! What a server remembers across a full stop: which terminal panes it had, so they can be reopened (fresh shells
//! in the same folders; running processes are not restored). `session.json` is the newest; up to 48 snapshots are
//! kept under `session-snapshots/` and 3 older copies under `session-backups/`. A file that cannot be read is never
//! overwritten: the newest backup that can be read is used and the failure is reported (R3.4, R3.5).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const MAX_SNAPSHOTS: usize = 48;
pub const MAX_BACKUPS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPane {
    pub id: String,
    pub workspace: String,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFile {
    pub version: u32,
    #[serde(default)]
    pub panes: Vec<SavedPane>,
}

impl Default for SessionFile {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            panes: Vec::new(),
        }
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join("session.json")
}

fn backup(dir: &Path, n: usize) -> PathBuf {
    dir.join("session-backups")
        .join(format!("session.{n}.json"))
}

fn sorted_snapshots(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("session-snapshots"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Saves the session: the previous file becomes the newest backup (older ones shift down, the oldest is dropped),
/// the new file is written atomically, and a snapshot is kept.
pub fn save(dir: &Path, session: &SessionFile) -> Result<(), String> {
    std::fs::create_dir_all(dir.join("session-backups")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("session-snapshots")).map_err(|e| e.to_string())?;
    if file(dir).is_file() {
        for n in (1..MAX_BACKUPS).rev() {
            if backup(dir, n).is_file() {
                // A failed shift loses only an old backup.
                let _ = std::fs::rename(backup(dir, n), backup(dir, n + 1));
            }
        }
        let _ = std::fs::copy(file(dir), backup(dir, 1));
    }
    vivianne_ai::paths::write_json_atomic(&file(dir), session)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    // Zero-padded, so the names sort in time order.
    vivianne_ai::paths::write_json_atomic(
        &dir.join("session-snapshots")
            .join(format!("{stamp:020}.json")),
        session,
    )?;
    let snapshots = sorted_snapshots(dir);
    for old in &snapshots[..snapshots.len().saturating_sub(MAX_SNAPSHOTS)] {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

fn read(path: &Path) -> Result<SessionFile, String> {
    let session: SessionFile =
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    // A newer format is not understood, so it is not used (and not rewritten).
    if session.version > FORMAT_VERSION {
        return Err(format!(
            "session file version {} is newer than this server reads ({FORMAT_VERSION})",
            session.version
        ));
    }
    Ok(session)
}

/// Loads the session. Nothing saved is an empty session. If `session.json` cannot be read, the newest readable
/// backup is used and `Err`-free; the problem is returned beside it so it can be logged.
#[must_use]
pub fn load(dir: &Path) -> (SessionFile, Option<String>) {
    if !file(dir).is_file() {
        return (SessionFile::default(), None);
    }
    match read(&file(dir)) {
        Ok(session) => (session, None),
        Err(why) => {
            for n in 1..=MAX_BACKUPS {
                if let Ok(session) = read(&backup(dir, n)) {
                    return (
                        session,
                        Some(format!(
                            "session.json could not be read ({why}); used backup {n}"
                        )),
                    );
                }
            }
            (
                SessionFile::default(),
                Some(format!(
                    "session.json could not be read ({why}) and no backup could either; starting empty"
                )),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("viv-persist-{}", uuid::Uuid::new_v4()))
    }

    fn with(ids: &[&str]) -> SessionFile {
        SessionFile {
            version: FORMAT_VERSION,
            panes: ids
                .iter()
                .map(|id| SavedPane {
                    id: (*id).to_owned(),
                    workspace: "w".into(),
                    command: None,
                })
                .collect(),
        }
    }

    #[test]
    fn a_saved_session_loads_back_and_nothing_saved_is_empty() {
        let dir = temp();
        assert_eq!(load(&dir), (SessionFile::default(), None));
        save(&dir, &with(&["a", "b"])).expect("saves");
        assert_eq!(load(&dir), (with(&["a", "b"]), None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_falls_back_to_the_newest_backup_and_is_never_silently_replaced() {
        let dir = temp();
        save(&dir, &with(&["one"])).expect("saves");
        save(&dir, &with(&["two"])).expect("saves");
        std::fs::write(file(&dir), "{ not json").expect("corrupts");
        let (session, problem) = load(&dir);
        assert_eq!(
            session,
            with(&["one"]),
            "the backup holds the save before the broken one"
        );
        assert!(problem.expect("reported").contains("used backup 1"));
        // The unreadable file is still there to look at.
        assert_eq!(
            std::fs::read_to_string(file(&dir)).expect("reads"),
            "{ not json"
        );
        std::fs::remove_file(backup(&dir, 1)).expect("removes");
        assert!(load(&dir).1.expect("reported").contains("starting empty"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backups_and_snapshots_are_capped_and_a_newer_format_is_refused() {
        let dir = temp();
        for i in 0..MAX_SNAPSHOTS + 6 {
            save(&dir, &with(&[&i.to_string()])).expect("saves");
        }
        assert_eq!(sorted_snapshots(&dir).len(), MAX_SNAPSHOTS);
        let backups = std::fs::read_dir(dir.join("session-backups"))
            .expect("lists")
            .count();
        assert_eq!(backups, MAX_BACKUPS);
        std::fs::write(file(&dir), r#"{"version": 99, "panes": []}"#).expect("writes");
        let (_, problem) = load(&dir);
        assert!(problem.expect("reported").contains("newer"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
