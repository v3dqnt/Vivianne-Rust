//! The files: events appended to `telemetry.jsonl`, one crash per file in `crashes/`. Same names and shapes as
//! the old app, so what it recorded is read in place.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vivianne_ai::paths::write_json_atomic;

use crate::props;

/// Events kept on disk.
const MAX_EVENTS: usize = 2_000;
/// Crash files kept; the oldest are deleted beyond this.
const MAX_CRASHES: usize = 20;
/// The longest line an event is written as (kind and properties are capped, so this is a ceiling, not a target).
const MAX_LINE_BYTES: usize = 700;
/// The file is only looked at for trimming once it is this large: the cap's worth of longest lines, plus
/// room to grow. A trim happens once in hundreds of appends, never on every one.
const TRIM_AT_BYTES: u64 = (MAX_EVENTS * MAX_LINE_BYTES) as u64 + 256 * 1024;

/// One process-wide lock around every append and trim, so two threads never interleave their lines or trim
/// over each other.
static APPEND: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryEvent {
    /// Unix millis.
    pub ts: u64,
    /// Dotted name, e.g. `turn.complete`.
    pub kind: String,
    /// Numbers, booleans and short identifiers only (see `props`). Older files may hold anything.
    #[serde(default)]
    pub props: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrashRecord {
    pub ts: u64,
    pub message: String,
    pub location: String,
    pub backtrace: String,
    pub app_version: String,
    pub os: String,
}

/// How much is kept and when it is trimmed. A struct so the tests can use small numbers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub max_events: usize,
    pub trim_at_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_events: MAX_EVENTS, trim_at_bytes: TRIM_AT_BYTES }
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The folder the files live in (`~/.viv/agent`), or any folder for a test.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    dir: PathBuf,
    limits: Limits,
}

impl Store {
    pub(crate) fn standard() -> Result<Self, String> {
        Ok(Self::at(vivianne_ai::paths::viv_agent_dir()?))
    }

    pub(crate) fn at(dir: PathBuf) -> Self {
        Self { dir, limits: Limits::default() }
    }

    #[cfg(test)]
    pub(crate) fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    fn events_path(&self) -> PathBuf {
        self.dir.join("telemetry.jsonl")
    }

    fn crashes_dir(&self) -> PathBuf {
        self.dir.join("crashes")
    }

    /// Records one event: the kind and properties are cleaned first, so what is written is never free text.
    pub(crate) fn track(&self, kind: &str, props: &Value) -> Result<(), String> {
        let kind = props::clean_kind(kind).ok_or_else(|| format!("'{kind}' is not an event name"))?;
        let event = TelemetryEvent { ts: now_ms(), kind: kind.to_owned(), props: props::clean(props) };
        self.append(&event)
    }

    fn append(&self, event: &TelemetryEvent) -> Result<(), String> {
        let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        let _guard = APPEND.lock().unwrap_or_else(PoisonError::into_inner);
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("folder: {e}"))?;
        let path = self.events_path();
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| format!("open: {e}"))?;
        // One write per line, so a reader never sees half of one.
        file.write_all(format!("{line}\n").as_bytes()).map_err(|e| format!("write: {e}"))?;
        // The size is a stat call, not a read: the file is only opened again when it has grown well past the cap.
        let size = file.metadata().map_or(0, |m| m.len());
        drop(file);
        if size > self.limits.trim_at_bytes {
            self.trim(&path);
        }
        Ok(())
    }

    /// Keeps the newest events: at most `max_events`, and at most half the trim size in bytes, so one trim is
    /// followed by many appends and an old file with long lines cannot be trimmed on every one.
    fn trim(&self, path: &Path) {
        let Ok(contents) = std::fs::read_to_string(path) else { return };
        let budget = usize::try_from(self.limits.trim_at_bytes / 2).unwrap_or(usize::MAX);
        let (mut kept, mut bytes) = (Vec::new(), 0_usize);
        for line in contents.lines().rev().take(self.limits.max_events) {
            bytes += line.len() + 1;
            if bytes > budget && !kept.is_empty() {
                break;
            }
            kept.push(line);
        }
        kept.reverse();
        let tmp = path.with_extension("tmp");
        let written = std::fs::write(&tmp, format!("{}\n", kept.join("\n"))).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            log::debug!("[telemetry] could not trim: {e}");
        }
    }

    pub(crate) fn read_events(&self) -> Vec<TelemetryEvent> {
        let Ok(contents) = std::fs::read_to_string(self.events_path()) else { return Vec::new() };
        contents.lines().filter_map(|line| serde_json::from_str::<TelemetryEvent>(line).ok()).collect()
    }

    /// Writes a crash through a temp file and a rename, and forgets the oldest beyond the cap.
    pub(crate) fn write_crash(&self, record: &CrashRecord) -> Result<(), String> {
        let dir = self.crashes_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("crashes folder: {e}"))?;
        write_json_atomic(&dir.join(format!("crash-{}.json", record.ts)), record)?;
        prune_crashes(&dir);
        Ok(())
    }

    /// Newest first.
    pub(crate) fn read_crashes(&self) -> Vec<CrashRecord> {
        let mut files = crash_files(&self.crashes_dir());
        files.reverse();
        files.iter().filter_map(|p| std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<CrashRecord>(&s).ok())).collect()
    }

    /// Deletes every event and crash. It really deletes: the user owns this data.
    pub(crate) fn clear(&self) -> Result<(), String> {
        let _guard = APPEND.lock().unwrap_or_else(PoisonError::into_inner);
        let mut failed = Vec::new();
        let events = self.events_path();
        let files = std::iter::once(events).chain(crash_files(&self.crashes_dir()));
        for file in files {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failed.push(format!("{}: {e}", file.display())),
            }
        }
        if failed.is_empty() { Ok(()) } else { Err(format!("Could not delete: {}", failed.join("; "))) }
    }
}

/// The crash files, oldest first. Their names carry the time (`crash-<millis>.json`); they are ordered by that
/// number, not by name, since "crash-10" sorts before "crash-9" as text.
fn crash_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort_by_key(|p| (crash_time(p), p.clone()));
    files
}

/// The time in a crash file's name, or 0 for a name that has none.
fn crash_time(path: &Path) -> u64 {
    path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.strip_prefix("crash-")).and_then(|n| n.parse().ok()).unwrap_or(0)
}

fn prune_crashes(dir: &Path) {
    let files = crash_files(dir);
    if files.len() > MAX_CRASHES {
        for stale in &files[..files.len() - MAX_CRASHES] {
            let _ = std::fs::remove_file(stale);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!("vivianne-telemetry-{}-{}", std::process::id(), now_ms()));
        let dir = dir.join(format!("{:?}", std::thread::current().id()).replace(['(', ')'], ""));
        Store::at(dir)
    }

    fn crash(ts: u64) -> CrashRecord {
        CrashRecord { ts, message: "boom".into(), location: "a.rs:1:1".into(), backtrace: "trace".into(), app_version: "0.0.1".into(), os: "windows x86_64".into() }
    }

    #[test]
    fn an_event_is_one_line_with_clean_props_and_is_read_back() {
        let store = temp_store();
        store.track("turn.complete", &json!({ "provider": "openai", "ms": 900, "note": "free text is not kept" })).unwrap();
        let text = std::fs::read_to_string(store.events_path()).unwrap();
        assert_eq!(text.lines().count(), 1);
        let events = store.read_events();
        assert_eq!(events[0].kind, "turn.complete");
        assert_eq!(events[0].props, json!({ "provider": "openai", "ms": 900 }));
        assert!(store.track("Not A Kind", &json!({})).is_err());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn lines_from_many_threads_never_interleave() {
        let store = temp_store();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for n in 0..60 {
                        store.track("test.event", &json!({ "thread": t, "n": n })).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let text = std::fs::read_to_string(store.events_path()).unwrap();
        assert_eq!(text.lines().count(), 480);
        assert_eq!(store.read_events().len(), 480, "every line is a whole event");
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn the_file_is_trimmed_only_once_it_is_well_past_its_size_and_keeps_the_newest() {
        let limits = Limits { max_events: 10, trim_at_bytes: 4_000 };
        let store = temp_store().with_limits(limits);
        for n in 0..30 {
            store.track("test.event", &json!({ "n": n })).unwrap();
        }
        // About 70 bytes a line: 30 lines are under the trim size, so nothing was trimmed.
        assert_eq!(store.read_events().len(), 30);
        for n in 30..120 {
            store.track("test.event", &json!({ "n": n })).unwrap();
        }
        let events = store.read_events();
        assert!(events.len() <= 10 + 60, "trimmed to the cap, then grew a little: {}", events.len());
        assert_eq!(events.last().and_then(|e| e.props["n"].as_u64()), Some(119), "the newest event is kept");
        assert!(events.len() < 120, "the oldest were dropped");
        assert!(!store.events_path().with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn crashes_are_written_whole_newest_first_and_pruned_to_the_cap() {
        let store = temp_store();
        for ts in 1..=(MAX_CRASHES as u64 + 5) {
            store.write_crash(&crash(ts)).unwrap();
        }
        let crashes = store.read_crashes();
        assert_eq!(crashes.len(), MAX_CRASHES);
        assert_eq!(crashes[0].ts, MAX_CRASHES as u64 + 5);
        assert!(std::fs::read_dir(store.crashes_dir()).unwrap().flatten().all(|e| e.path().extension().is_some_and(|x| x == "json")), "no temp file is left");
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn clear_deletes_events_and_crashes_and_is_fine_when_there_are_none() {
        let store = temp_store();
        store.clear().unwrap();
        store.track("test.event", &json!({})).unwrap();
        store.write_crash(&crash(5)).unwrap();
        store.clear().unwrap();
        assert!(store.read_events().is_empty());
        assert!(store.read_crashes().is_empty());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn lines_the_old_app_wrote_are_still_read() {
        let store = temp_store();
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.events_path(), "{\"ts\":1,\"kind\":\"panel.open\",\"props\":{\"id\":\"usage\"}}\nnot json\n{\"ts\":2,\"kind\":\"x\"}\n").unwrap();
        let events = store.read_events();
        assert_eq!(events.len(), 2, "a damaged line is skipped, not fatal");
        assert_eq!(events[1].props, Value::Null);
        let _ = std::fs::remove_dir_all(store.dir());
    }
}
