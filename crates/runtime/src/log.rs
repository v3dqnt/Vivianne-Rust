//! The saved thread as an append-only log (format 2). Port of the idea behind the original `session-persistence-jsonl`:
//! a thread is one `<id>.jsonl` file whose first line says what it is and whose other lines are added to, never edited.
//!
//! - Line 1 is the header (`type: header`, the format, the generation, and the thread's settings).
//! - A message is one `type: message` line; a settings change is one `type: meta` line that replaces the earlier ones.
//! - A save that only adds messages appends them. A save that rewrote the earlier history (the conversation was
//!   condensed or rewound, so it is shorter) first keeps the old file as `<id>.g<N>.jsonl.gz`, immutable, and starts the
//!   next generation. A save that changed earlier messages without shortening it (old tool results were masked) rewrites
//!   the file in place, atomically.
//! - A file that ends in half a line (the process died mid-write) reads as everything before it; the next save rewrites it
//!   clean. A damaged line anywhere else is an error, and the file is set aside, never read as if it were whole.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vivianne_core::{Message, Mode};

use crate::formats;
use crate::store::{Parent, SavedThread};

/// The thread's settings as the header and `meta` lines carry them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Meta {
    workspace: String,
    title: String,
    title_source: String,
    provider: String,
    model: String,
    #[serde(default)]
    mode: Mode,
    updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<Parent>,
}

impl Meta {
    fn of(thread: &SavedThread) -> Self {
        Self {
            workspace: thread.workspace.clone(),
            title: thread.title.clone(),
            title_source: thread.title_source.clone(),
            provider: thread.provider.clone(),
            model: thread.model.clone(),
            mode: thread.mode,
            updated_at: thread.updated_at,
            parent: thread.parent.clone(),
        }
    }

    /// Whether anything but the time differs: the time alone moving is not a reason to write.
    fn differs_apart_from_time(&self, other: &Self) -> bool {
        Self {
            updated_at: 0,
            ..self.clone()
        } != Self {
            updated_at: 0,
            ..other.clone()
        }
    }
}

/// What a log file holds, read.
#[derive(Debug, Clone)]
struct Parsed {
    thread: SavedThread,
    generation: u32,
    /// One hash per message line, to tell a save that only adds from one that rewrites.
    hashes: Vec<u64>,
    /// Whether the file ends in a whole line.
    clean: bool,
}

/// What a read of a log came to, for the store.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub thread: SavedThread,
    pub generation: u32,
    /// The file ended in half a line, which was left out.
    pub torn_tail: bool,
}

/// What is known of the file as last read or written, so a save need not read it again.
#[derive(Debug, Clone)]
struct Disk {
    stamp: (u64, Option<SystemTime>),
    generation: u32,
    hashes: Vec<u64>,
    meta: Meta,
    clean: bool,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Disk>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Disk>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn remember(path: &Path, disk: Disk) {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.to_owned(), disk);
}

fn forget(path: &Path) {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(path);
}

fn stamp_of(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

fn hash_line(line: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// The path of a thread's active log.
#[must_use]
pub fn path_of(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.jsonl"))
}

fn generation_path(dir: &Path, id: &str, generation: u32) -> PathBuf {
    dir.join(format!("{id}.g{generation}.jsonl.gz"))
}

fn object_with(kind: &str, meta: &Meta, extra: Value) -> String {
    let mut value = serde_json::to_value(meta).unwrap_or_else(|_| json!({}));
    if let (Some(object), Some(more)) = (value.as_object_mut(), extra.as_object()) {
        object.insert("type".to_owned(), Value::from(kind));
        object.extend(more.clone());
    }
    value.to_string()
}

fn header_line(id: &str, generation: u32, meta: &Meta) -> String {
    object_with(
        "header",
        meta,
        json!({ "version": formats::CURRENT, "generation": generation, "id": id }),
    )
}

fn meta_line(meta: &Meta) -> String {
    object_with("meta", meta, json!({}))
}

/// Reads log text into a thread. A half line at the very end is dropped; a damaged line elsewhere is an error.
fn parse(text: &str) -> Result<Parsed, String> {
    let chunks: Vec<&str> = text.split_inclusive('\n').collect();
    let mut header: Option<(String, u32)> = None;
    let mut meta: Option<Meta> = None;
    let mut history = Vec::new();
    let mut hashes = Vec::new();
    let mut clean = true;
    for (i, chunk) in chunks.iter().enumerate() {
        let line = chunk.trim();
        if line.is_empty() {
            continue;
        }
        let last = i + 1 == chunks.len();
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) if last => {
                clean = false;
                break;
            }
            Err(e) => return Err(format!("line {} is damaged: {e}", i + 1)),
        };
        if last && !chunk.ends_with('\n') {
            clean = false;
        }
        match value["type"].as_str() {
            Some("header") if header.is_none() => {
                let version = value["version"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or("the header has no format version")?;
                if version > formats::CURRENT {
                    return Err(formats::FormatError::Unsupported { version }.to_string());
                }
                let id = value["id"]
                    .as_str()
                    .ok_or("the header has no id")?
                    .to_owned();
                let generation = value["generation"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(1);
                meta = Some(
                    serde_json::from_value(value)
                        .map_err(|e| format!("the header is not a thread's: {e}"))?,
                );
                header = Some((id, generation));
            }
            Some("meta") if header.is_some() => {
                meta = Some(
                    serde_json::from_value(value)
                        .map_err(|e| format!("line {} is not a settings line: {e}", i + 1))?,
                );
            }
            Some("message") if header.is_some() => {
                let message: Message = serde_json::from_value(value["message"].clone())
                    .map_err(|e| format!("line {} is not a message: {e}", i + 1))?;
                hashes.push(hash_line(
                    &serde_json::to_string(&message).map_err(|e| e.to_string())?,
                ));
                history.push(message);
            }
            _ if header.is_none() => {
                return Err("the file does not start with a header line".to_owned());
            }
            // A line kind a newer build adds is skipped, so what it wrote beside the messages does not hide them.
            _ => {}
        }
    }
    let (Some((id, generation)), Some(meta)) = (header, meta) else {
        return Err("the file has no header line".to_owned());
    };
    Ok(Parsed {
        thread: SavedThread {
            id,
            workspace: meta.workspace,
            title: meta.title,
            title_source: meta.title_source,
            provider: meta.provider,
            model: meta.model,
            mode: meta.mode,
            updated_at: meta.updated_at,
            parent: meta.parent,
            history,
        },
        generation,
        hashes,
        clean,
    })
}

/// Reads a thread's active log. `Ok(None)` when there is none.
pub fn read(dir: &Path, id: &str) -> Result<Option<Loaded>, String> {
    let path = path_of(dir, id);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let parsed = parse(&text)?;
    Ok(Some(Loaded {
        thread: parsed.thread,
        generation: parsed.generation,
        torn_tail: !parsed.clean,
    }))
}

/// Why the file on disk cannot be the base of a save.
enum Unusable {
    /// Written by a newer build: never touched.
    Newer(String),
    /// Damaged: set aside before a fresh one is written.
    Damaged,
}

fn disk_state(path: &Path) -> Result<Option<Disk>, Unusable> {
    let Some(stamp) = stamp_of(path) else {
        forget(path);
        return Ok(None);
    };
    if let Some(known) = cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        && known.stamp == stamp
    {
        return Ok(Some(known.clone()));
    }
    let text = std::fs::read_to_string(path).map_err(|_| Unusable::Damaged)?;
    let parsed = parse(&text).map_err(|why| {
        if why.contains("newer Vivianne") {
            Unusable::Newer(why)
        } else {
            Unusable::Damaged
        }
    })?;
    let disk = Disk {
        stamp,
        generation: parsed.generation,
        hashes: parsed.hashes,
        meta: Meta::of(&parsed.thread),
        clean: parsed.clean,
    };
    remember(path, disk.clone());
    Ok(Some(disk))
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn rewrite(
    path: &Path,
    thread: &SavedThread,
    generation: u32,
    lines: &[String],
    hashes: Vec<u64>,
) -> Result<(), String> {
    let meta = Meta::of(thread);
    let mut text = header_line(&thread.id, generation, &meta);
    text.push('\n');
    for line in lines {
        text.push_str(&format!("{{\"type\":\"message\",\"message\":{line}}}\n"));
    }
    write_atomic(path, &text)?;
    if let Some(stamp) = stamp_of(path) {
        remember(
            path,
            Disk {
                stamp,
                generation,
                hashes,
                meta,
                clean: true,
            },
        );
    }
    Ok(())
}

fn append(
    path: &Path,
    thread: &SavedThread,
    disk: &Disk,
    lines: &[String],
    hashes: Vec<u64>,
) -> Result<(), String> {
    let meta = Meta::of(thread);
    let added = &lines[disk.hashes.len()..];
    if added.is_empty() && !meta.differs_apart_from_time(&disk.meta) {
        return Ok(());
    }
    let mut text = String::new();
    for line in added {
        text.push_str(&format!("{{\"type\":\"message\",\"message\":{line}}}\n"));
    }
    text.push_str(&meta_line(&meta));
    text.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_data().map_err(|e| e.to_string())?;
    match stamp_of(path) {
        Some(stamp) => remember(
            path,
            Disk {
                stamp,
                generation: disk.generation,
                hashes,
                meta,
                clean: true,
            },
        ),
        None => forget(path),
    }
    Ok(())
}

/// Keeps the file as it is as generation `generation`: compressed, written once, never changed.
fn archive(dir: &Path, id: &str, generation: u32) -> Result<(), String> {
    let target = generation_path(dir, id, generation);
    if target.exists() {
        return Ok(());
    }
    let raw = std::fs::read(path_of(dir, id)).map_err(|e| e.to_string())?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).map_err(|e| e.to_string())?;
    let packed = encoder.finish().map_err(|e| e.to_string())?;
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, packed).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())
}

/// Makes `thread` the saved state: appends when it only grew, starts a new generation when its earlier history was
/// condensed or rewound, rewrites in place when earlier messages changed without shortening it.
pub fn write(dir: &Path, thread: &SavedThread) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = path_of(dir, &thread.id);
    let lines: Vec<String> = thread
        .history
        .iter()
        .map(|m| serde_json::to_string(m).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let hashes: Vec<u64> = lines.iter().map(|l| hash_line(l)).collect();
    let disk = match disk_state(&path) {
        Ok(disk) => disk,
        Err(Unusable::Newer(why)) => return Err(why),
        Err(Unusable::Damaged) => {
            // What cannot be read is kept for a person to look at; the conversation in memory is written fresh.
            let _ = std::fs::rename(&path, dir.join(format!("{}.jsonl.damaged", thread.id)));
            forget(&path);
            None
        }
    };
    let Some(disk) = disk else {
        return rewrite(&path, thread, 1, &lines, hashes);
    };
    let kept = disk.hashes.len();
    let only_grew = disk.clean && kept <= hashes.len() && hashes[..kept] == disk.hashes[..];
    if only_grew {
        append(&path, thread, &disk, &lines, hashes)
    } else if hashes.len() < kept {
        archive(dir, &thread.id, disk.generation)?;
        rewrite(&path, thread, disk.generation + 1, &lines, hashes)
    } else {
        rewrite(&path, thread, disk.generation, &lines, hashes)
    }
}

/// Where the thread was branched from, as the log last said. Read from what is already known of the file.
#[must_use]
pub fn parent_of(dir: &Path, id: &str) -> Option<Parent> {
    disk_state(&path_of(dir, id))
        .ok()
        .flatten()
        .and_then(|disk| disk.meta.parent)
}

/// The generations kept for a thread, oldest first.
#[must_use]
pub fn generations(dir: &Path, id: &str) -> Vec<u32> {
    let prefix = format!("{id}.g");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix)?
                .strip_suffix(".jsonl.gz")?
                .parse()
                .ok()
        })
        .collect();
    found.sort_unstable();
    found
}

/// One kept generation, as the thread stood when it was kept.
pub fn read_generation(
    dir: &Path,
    id: &str,
    generation: u32,
) -> Result<Option<SavedThread>, String> {
    let bytes = match std::fs::read(generation_path(dir, id, generation)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    GzDecoder::new(bytes.as_slice())
        .read_to_string(&mut text)
        .map_err(|e| format!("generation {generation} cannot be unpacked: {e}"))?;
    Ok(Some(parse(&text)?.thread))
}

/// Removes a thread's log and every generation kept for it.
pub fn remove(dir: &Path, id: &str) -> Result<(), String> {
    let path = path_of(dir, id);
    forget(&path);
    let mut doomed = vec![path];
    doomed.extend(
        generations(dir, id)
            .into_iter()
            .map(|g| generation_path(dir, id, g)),
    );
    for file in doomed {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
