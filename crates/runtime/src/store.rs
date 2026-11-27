//! Threads kept on disk under `~/.viv/agent/threads/`, so a conversation survives closing the app. A thread belongs to the
//! project folder it was started in. Each is an append-only log, `<id>.jsonl` (see [`crate::log`]); a thread an older
//! build saved as `<id>.json` is carried forward on first read and its source kept beside it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vivianne_core::{Message, Mode};

use crate::{formats, log};

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
    /// The conversation this one was branched from, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Parent>,
}

/// Where a branched conversation came from: the thread, and the turn it was cut before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub thread: String,
    pub turn: usize,
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

/// Starts a new thread from `id` as it was before its turn `before_turn` (1 is the whole conversation left out), to take
/// the conversation another way. The original is untouched; the new thread is saved and returned.
pub fn fork(id: &str, before_turn: usize) -> Result<SavedThread, String> {
    fork_in(&default_dir()?, id, before_turn)
}

fn fork_in(dir: &Path, id: &str, before_turn: usize) -> Result<SavedThread, String> {
    let original = load_in(dir, id).ok_or_else(|| format!("there is no saved thread {id}"))?;
    let history = crate::session::fork_before(&original.history, before_turn)
        .ok_or_else(|| format!("thread {id} has no turn {before_turn} to fork before"))?;
    let forked = SavedThread {
        id: uuid::Uuid::new_v4().to_string(),
        title: format!("{} (fork)", original.title),
        // A name the person did not choose would be replaced by a generated one; this one says what it is.
        title_source: "user".to_owned(),
        updated_at: now(),
        history,
        parent: Some(Parent {
            thread: original.id.clone(),
            turn: before_turn,
        }),
        ..original
    };
    save_in(dir, &forked)?;
    Ok(forked)
}

fn save_in(dir: &Path, thread: &SavedThread) -> Result<(), String> {
    if !valid_id(&thread.id) {
        return Err(format!("not a thread id: {}", thread.id));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // A file written by a newer Vivianne is not written over by an older one: that would drop what only it knows.
    let legacy = legacy_path(dir, &thread.id);
    let has_log = log::path_of(dir, &thread.id).exists();
    if !has_log
        && let Some(Ok(value)) = std::fs::read_to_string(&legacy).ok().map(|t| serde_json::from_str::<Value>(&t))
        && let formats::Classification::Unsupported { version } = formats::classify(&value)
    {
        return Err(formats::FormatError::Unsupported { version }.to_string());
    }
    // What the desktop showed is not kept.
    // A save from a screen that does not know the thread is a branch must not forget where it came from.
    let parent = thread
        .parent
        .clone()
        .or_else(|| log::parent_of(dir, &thread.id));
    let kept = SavedThread {
        history: crate::history::without_desktop_content(&thread.history),
        parent,
        ..thread.clone()
    };
    log::write(dir, &kept)?;
    if !has_log {
        retire_legacy(dir, &thread.id, 1);
    }
    Ok(())
}

fn legacy_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// The one-file form an older build wrote is kept as `<id>.v<N>.bak` once the log holds the thread.
fn retire_legacy(dir: &Path, id: &str, from: u32) {
    let legacy = legacy_path(dir, id);
    let backup = dir.join(format!("{id}.v{from}.bak"));
    if backup.exists() {
        let _ = std::fs::remove_file(&legacy);
    } else {
        let _ = std::fs::rename(&legacy, &backup);
    }
}

/// The ids of the threads in `dir`: every `<id>.jsonl` and every older `<id>.json`, each once.
fn ids_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let ids: std::collections::BTreeSet<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json" || x == "jsonl"))
        .filter_map(|p| Some(p.file_stem()?.to_str()?.to_owned()))
        // `<id>.proj.json` and the like are not threads.
        .filter(|id| valid_id(id))
        .collect();
    ids.into_iter().collect()
}

/// Saves into `dir` (for the query module's tests).
#[cfg(test)]
pub(crate) fn save_to(dir: &Path, thread: &SavedThread) -> Result<(), String> {
    save_in(dir, thread)
}

/// Deletes a thread inside `dir` (for the query module's tests).
#[cfg(test)]
pub(crate) fn delete_to(dir: &Path, id: &str) -> Result<(), String> {
    delete_in(dir, id)
}

/// Branches a thread inside `dir` (for the query module's tests).
#[cfg(test)]
pub(crate) fn fork_to(dir: &Path, id: &str, before_turn: usize) -> Result<SavedThread, String> {
    fork_in(dir, id, before_turn)
}

/// Every readable thread in `dir`, in id order. For the query module; one that cannot be read is left out.
pub(crate) fn all_in(dir: &Path) -> Vec<SavedThread> {
    ids_in(dir)
        .into_iter()
        .filter_map(|id| load_in(dir, &id))
        .collect()
}

/// One thread in `dir`, with why it could not be read.
pub(crate) fn read_thread_in(dir: &Path, id: &str) -> Result<Option<SavedThread>, String> {
    read_in(dir, id)
}

/// The generation the thread's active log is on (1 until a history has been condensed or rewound).
pub(crate) fn generation_in(dir: &Path, id: &str) -> u32 {
    log::read(dir, id)
        .ok()
        .flatten()
        .map_or(1, |loaded| loaded.generation)
}

/// The directory threads are kept in.
pub(crate) fn threads_dir() -> Result<PathBuf, String> {
    default_dir()
}

/// The generations kept for a thread (the older states of its log), oldest first.
#[must_use]
pub fn generations(id: &str) -> Vec<u32> {
    default_dir().map(|dir| log::generations(&dir, id)).unwrap_or_default()
}

/// A kept generation, as the thread stood when it was condensed or rewound.
pub fn load_generation(id: &str, generation: u32) -> Result<Option<SavedThread>, String> {
    if !valid_id(id) {
        return Ok(None);
    }
    log::read_generation(&default_dir()?, id, generation)
}

/// Why a saved file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub why: String,
}

/// Reads one saved thread, carrying an older format forward (the original file is kept beside it as `<id>.v<N>.bak`
/// the first time) and saying why when it cannot be read. `Ok(None)` when there is no such file.
fn read_in(dir: &Path, id: &str) -> Result<Option<SavedThread>, String> {
    if !valid_id(id) {
        return Ok(None);
    }
    if let Some(read) = log::read(dir, id)? {
        return Ok(Some(read.thread));
    }
    let path = legacy_path(dir, id);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let parsed: Value = serde_json::from_str(&text).map_err(|e| format!("not valid JSON: {e}"))?;
    let restored = formats::restore(parsed).map_err(|e| e.to_string())?;
    let thread: SavedThread = serde_json::from_value(restored.value.clone()).map_err(|e| format!("not a saved conversation: {e}"))?;
    // Carried forward on disk too, as a log, with the source kept: nothing is lost if the new form turns out wrong.
    let from = restored.migrated_from.unwrap_or(1);
    let backup = dir.join(format!("{id}.v{from}.bak"));
    if !backup.exists() {
        let _ = std::fs::write(&backup, &text);
    }
    if log::write(dir, &thread).is_ok() {
        retire_legacy(dir, id, from);
    }
    Ok(Some(thread))
}

fn load_in(dir: &Path, id: &str) -> Option<SavedThread> {
    read_in(dir, id).ok().flatten()
}

/// The saved files that cannot be read, each with the reason: damaged, or written by a newer Vivianne. They are never
/// deleted or rewritten, only reported (`viv doctor`).
#[must_use]
pub fn problems() -> Vec<Problem> {
    default_dir().map(|dir| problems_in(&dir)).unwrap_or_default()
}

fn problems_in(dir: &Path) -> Vec<Problem> {
    let mut found: Vec<Problem> = ids_in(dir)
        .into_iter()
        .filter_map(|id| {
            let file = if log::path_of(dir, &id).exists() { format!("{id}.jsonl") } else { format!("{id}.json") };
            read_in(dir, &id).err().map(|why| Problem { file, why })
        })
        .collect();
    found.sort_by(|a, b| a.file.cmp(&b.file));
    found
}

/// The right to write `id` while a run works on it; another process that asks is refused until this is dropped.
pub fn lease(id: &str) -> Result<crate::lease::Lease, crate::lease::LeaseError> {
    if !valid_id(id) {
        return Err(crate::lease::LeaseError::Io(format!("not a thread id: {id}")));
    }
    let dir = default_dir().map_err(crate::lease::LeaseError::Io)?;
    crate::lease::Lease::take(&dir, id)
}

/// Makes the conversation as it stands durable, in the middle of a run: the history is written now, so a crash loses
/// at most the step in flight. An existing thread keeps its title and settings; a first checkpoint makes the thread.
pub fn checkpoint(id: &str, workspace: &str, provider: &str, model: &str, mode: Mode, history: &[Message]) -> Result<(), String> {
    checkpoint_in(&default_dir()?, id, workspace, provider, model, mode, history)
}

fn checkpoint_in(dir: &Path, id: &str, workspace: &str, provider: &str, model: &str, mode: Mode, history: &[Message]) -> Result<(), String> {
    let thread = match read_in(dir, id)? {
        Some(existing) => SavedThread { history: history.to_vec(), updated_at: now(), ..existing },
        None => SavedThread {
            id: id.to_owned(),
            workspace: workspace.to_owned(),
            title: crate::title::first_ask(history).map_or_else(|| "New conversation".to_owned(), crate::title::fallback_title),
            title_source: "auto".to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            mode,
            updated_at: now(),
            history: history.to_vec(),
            parent: None,
        },
    };
    save_in(dir, &thread)
}

/// Every saved thread, with the project folder it was started in, newest first. For a remote client that does not
/// know which projects this machine has.
#[must_use]
pub fn list_everywhere() -> Vec<(String, Summary)> {
    let Ok(dir) = default_dir() else {
        return Vec::new();
    };
    let mut found: Vec<(String, Summary)> = ids_in(&dir)
        .into_iter()
        .filter_map(|id| load_in(&dir, &id))
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
    let mut found: Vec<Summary> = ids_in(dir)
        .into_iter()
        .filter_map(|id| load_in(dir, &id))
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
    log::remove(dir, id)?;
    crate::projection::forget(dir, id);
    match std::fs::remove_file(legacy_path(dir, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use vivianne_core::Role;

    use super::*;

    fn fresh_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-store-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    const ID: &str = "22222222-2222-2222-2222-222222222222";

    fn write_raw(dir: &Path, id: &str, json: &str) {
        std::fs::write(dir.join(format!("{id}.json")), json).expect("raw file");
    }

    #[test]
    fn what_is_saved_says_which_format_it_is() {
        let dir = fresh_dir("stamp");
        save_in(&dir, &thread(ID, "/w", 1)).expect("saved");
        let first_line = std::fs::read_to_string(dir.join(format!("{ID}.jsonl"))).expect("file");
        let header: Value = serde_json::from_str(first_line.lines().next().expect("a header")).expect("json");
        assert_eq!((header["type"].as_str(), header["version"].as_u64()), (Some("header"), Some(u64::from(formats::CURRENT))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_thread_saved_before_formats_had_versions_is_carried_forward_and_its_source_is_kept() {
        let dir = fresh_dir("migrate");
        let old = format!(r#"{{"id":"{ID}","workspace":"/w","title":"Old","title_source":"auto","provider":"p","model":"m","updated_at":7,"history":[{{"role":"user","text":"hi"}}]}}"#);
        write_raw(&dir, ID, &old);
        let loaded = load_in(&dir, ID).expect("an older file still opens");
        assert_eq!((loaded.title.as_str(), loaded.history.len()), ("Old", 1));
        assert!(dir.join(format!("{ID}.jsonl")).exists() && !dir.join(format!("{ID}.json")).exists(), "kept as a log now");
        assert_eq!(std::fs::read_to_string(dir.join(format!("{ID}.v0.bak"))).expect("the source is kept"), old);
        // A second read changes nothing and does not touch the kept source.
        assert!(load_in(&dir, ID).is_some());
        assert_eq!(std::fs::read_to_string(dir.join(format!("{ID}.v0.bak"))).expect("backup"), old);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_thread_from_a_newer_vivianne_is_neither_read_nor_written_over_and_is_reported() {
        let dir = fresh_dir("newer");
        let newer = format!(r#"{{"id":"{ID}","workspace":"/w","history":[],"version":99,"only_the_future_knows":true}}"#);
        write_raw(&dir, ID, &newer);
        assert!(load_in(&dir, ID).is_none());
        let refusal = save_in(&dir, &thread(ID, "/w", 1)).unwrap_err();
        assert!(refusal.contains("newer Vivianne"), "{refusal}");
        assert_eq!(std::fs::read_to_string(dir.join(format!("{ID}.json"))).expect("file"), newer, "left exactly as it was");
        let found = problems_in(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].file.starts_with(ID) && found[0].why.contains("newer Vivianne"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_is_reported_by_name_and_does_not_hide_the_others() {
        let dir = fresh_dir("damaged");
        write_raw(&dir, "33333333-3333-3333-3333-333333333333", "{ not json");
        write_raw(&dir, "44444444-4444-4444-4444-444444444444", r#"{"id":"x"}"#);
        let mut fine = thread(ID, "/w", 5);
        fine.history = vec![Message::new(Role::User, "hi")];
        save_in(&dir, &fine).expect("saved");
        let found = problems_in(&dir);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|p| p.why.contains("not valid JSON")) && found.iter().any(|p| p.why.contains("workspace") || p.why.contains("history")));
        assert_eq!(list_in(&dir, "/w").len(), 1, "the readable thread is still listed");
        assert!(read_in(&dir, "nobody").expect("no file is not an error").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_thread_an_earlier_build_saved_as_one_file_opens_and_becomes_a_log() {
        let dir = fresh_dir("one-file");
        let current = format!(r#"{{"id":"{ID}","workspace":"/w","title":"T","title_source":"auto","provider":"p","model":"m","updated_at":3,"history":[{{"role":"user","text":"hi"}}],"version":1}}"#);
        write_raw(&dir, ID, &current);
        assert_eq!(load_in(&dir, ID).expect("opens").history.len(), 1);
        assert!(dir.join(format!("{ID}.jsonl")).exists() && dir.join(format!("{ID}.v1.bak")).exists());
        assert_eq!(list_in(&dir, "/w").len(), 1, "listed once, not as both forms");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_thread_removes_its_log_and_every_kept_generation() {
        let dir = fresh_dir("delete-all");
        let mut long = thread(ID, "/w", 1);
        long.history = (0..4).map(|i| Message::new(Role::User, format!("m{i}"))).collect();
        save_in(&dir, &long).expect("saved");
        save_in(&dir, &thread(ID, "/w", 2)).expect("condensed");
        assert_eq!(log::generations(&dir, ID), [1]);
        delete_in(&dir, ID).expect("deleted");
        assert!(log::generations(&dir, ID).is_empty() && load_in(&dir, ID).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checkpoint_makes_the_thread_then_updates_it_without_touching_its_title() {
        let dir = fresh_dir("checkpoint");
        let first = vec![Message::new(Role::User, "fix the parser please")];
        checkpoint_in(&dir, ID, "/w", "p", "m", Mode::Auto, &first).expect("the first checkpoint makes the thread");
        let made = load_in(&dir, ID).expect("exists");
        assert_eq!((made.workspace.as_str(), made.history.len(), made.model.as_str()), ("/w", 1, "m"));
        assert!(made.title.to_lowercase().contains("parser"), "{}", made.title);
        let mut renamed = made.clone();
        renamed.title = "My name for it".to_owned();
        renamed.title_source = "user".to_owned();
        save_in(&dir, &renamed).expect("saved");
        let more = vec![first[0].clone(), Message::assistant("done".to_owned(), Vec::new())];
        checkpoint_in(&dir, ID, "/elsewhere", "other", "other", Mode::Plan, &more).expect("updates");
        let updated = load_in(&dir, ID).expect("exists");
        assert_eq!((updated.history.len(), updated.title.as_str(), updated.title_source.as_str()), (2, "My name for it", "user"));
        assert_eq!((updated.workspace.as_str(), updated.model.as_str()), ("/w", "m"), "an existing thread keeps its own settings");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checkpoint_never_writes_over_a_thread_it_cannot_read() {
        let dir = fresh_dir("checkpoint-newer");
        let newer = format!(r#"{{"id":"{ID}","workspace":"/w","history":[],"version":99}}"#);
        write_raw(&dir, ID, &newer);
        assert!(checkpoint_in(&dir, ID, "/w", "p", "m", Mode::Auto, &[Message::new(Role::User, "hi")]).is_err());
        assert_eq!(std::fs::read_to_string(dir.join(format!("{ID}.json"))).expect("file"), newer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fork_is_a_new_thread_with_the_earlier_turns_and_the_original_is_untouched() {
        let dir = std::env::temp_dir().join(format!("viv-fork-{}", uuid::Uuid::new_v4().simple()));
        let mut original = thread("11111111-1111-1111-1111-111111111111", "/w", 5);
        original.title = "Fix the parser".to_owned();
        original.history = vec![
            Message::new(Role::User, "one"),
            Message::assistant("a".to_owned(), Vec::new()),
            Message::new(Role::User, "two"),
            Message::assistant("b".to_owned(), Vec::new()),
        ];
        save_in(&dir, &original).expect("saved");
        let forked = fork_in(&dir, &original.id, 2).expect("forked");
        assert_ne!(forked.id, original.id);
        assert_eq!(forked.title, "Fix the parser (fork)");
        assert_eq!(forked.history.len(), 2);
        assert_eq!(load_in(&dir, &forked.id).expect("saved fork").history.len(), 2);
        assert_eq!(load_in(&dir, &original.id).expect("original").history.len(), 4);
        assert!(fork_in(&dir, &original.id, 3).is_err() && fork_in(&dir, "nope", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
            parent: None,
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

#[cfg(test)]
mod parent_tests {
    use super::*;

    #[test]
    fn a_branch_keeps_its_parent_through_saves_that_do_not_know_about_it() {
        let dir = std::env::temp_dir().join(format!("viv-parent-{}", uuid::Uuid::new_v4().simple()));
        let original = SavedThread {
            id: "11111111-1111-1111-1111-111111111111".into(),
            workspace: "/w".into(),
            title: "t".into(),
            title_source: "auto".into(),
            provider: "p".into(),
            model: "m".into(),
            mode: Mode::Ask,
            updated_at: 1,
            history: vec![
                Message::new(user_role(), "one"),
                Message::assistant("a".to_owned(), Vec::new()),
                Message::new(user_role(), "two"),
            ],
            parent: None,
        };
        save_in(&dir, &original).expect("saved");
        let forked = fork_in(&dir, &original.id, 2).expect("forked");
        assert_eq!(forked.parent, Some(Parent { thread: original.id.clone(), turn: 2 }));
        // What a screen saves: the same thread, built without a parent.
        let mut resaved = forked.clone();
        resaved.parent = None;
        resaved.history.push(Message::new(user_role(), "more"));
        save_in(&dir, &resaved).expect("saved again");
        assert_eq!(load_in(&dir, &forked.id).expect("exists").parent, forked.parent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn user_role() -> vivianne_core::Role {
        vivianne_core::Role::User
    }
}
