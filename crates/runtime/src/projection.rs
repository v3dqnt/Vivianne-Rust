//! Views of a conversation that are folds of its history, kept up to date as the history grows. Port of the original
//! `session-projection` (a registry of named folds) and `session-projection-cache` (their state saved beside the thread).
//!
//! Each projection takes messages one at a time and can say what it knows so far. The registry runs every one of them over
//! a thread. The cache keeps each projection's state in `<id>.proj.json` together with how many messages it covers and a
//! digest of exactly those messages, so the next read folds only what was added since. The cache is trusted only when the
//! digest of the saved prefix still matches the history (a condensed or edited history refolds from the start) and every
//! projection's version is the one in this build; anything else is discarded and rebuilt, never read as current.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use vivianne_core::Message;

use crate::session_stats::{OutlineFold, StatsFold};

/// One named fold over a history.
pub trait Projection {
    /// What the cache calls it; never changes.
    fn name(&self) -> &'static str;
    /// Raised whenever the fold's rules or its saved state change shape, which throws old saved state away.
    fn version(&self) -> u32;
    fn push(&mut self, message: &Message);
    /// The state to save.
    fn save(&self) -> Value;
    /// Takes up saved state; `false` when it is not what this fold wrote (it then starts empty).
    fn load(&mut self, state: Value) -> bool;
    /// What the fold knows so far.
    fn view(&self) -> Value;
}

impl Projection for StatsFold {
    fn name(&self) -> &'static str {
        "stats"
    }
    fn version(&self) -> u32 {
        1
    }
    fn push(&mut self, message: &Message) {
        StatsFold::push(self, message);
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
    fn load(&mut self, state: Value) -> bool {
        serde_json::from_value(state)
            .map(|fold| *self = fold)
            .is_ok()
    }
    fn view(&self) -> Value {
        serde_json::to_value(self.finish()).unwrap_or(Value::Null)
    }
}

impl Projection for OutlineFold {
    fn name(&self) -> &'static str {
        "outline"
    }
    fn version(&self) -> u32 {
        1
    }
    fn push(&mut self, message: &Message) {
        OutlineFold::push(self, message);
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
    fn load(&mut self, state: Value) -> bool {
        serde_json::from_value(state)
            .map(|fold| *self = fold)
            .is_ok()
    }
    fn view(&self) -> Value {
        serde_json::to_value(self.finish()).unwrap_or(Value::Null)
    }
}

/// The projections this build knows, all fresh.
#[must_use]
pub fn registry() -> Vec<Box<dyn Projection>> {
    vec![
        Box::new(StatsFold::default()),
        Box::new(OutlineFold::default()),
    ]
}

/// What is saved in `<id>.proj.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    /// How many messages of the history the states cover.
    upto: usize,
    /// SHA-256 of those messages, serialized in order.
    digest: String,
    /// Each projection's version and state, by name.
    states: std::collections::BTreeMap<String, Saved>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    version: u32,
    state: Value,
}

/// What a projection run came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Projected {
    /// Each projection's view, by name.
    pub views: std::collections::BTreeMap<String, Value>,
    /// How many messages were folded by this call (the whole history when the cache did not apply).
    pub folded: usize,
}

fn digest_of(history: &[Message]) -> String {
    let mut sha = Sha256::new();
    for message in history {
        sha.update(
            serde_json::to_string(message)
                .unwrap_or_default()
                .as_bytes(),
        );
        sha.update(b"\n");
    }
    format!("{:x}", sha.finalize())
}

fn cache_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.proj.json"))
}

fn read_cache(path: &Path) -> Option<Cache> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Runs `projections` over `history`, starting from the cached state when it still applies and saving the new state.
/// A cache that cannot be written is not an error: the views are right either way, only slower the next time.
#[must_use]
pub fn project_in(
    dir: &Path,
    id: &str,
    history: &[Message],
    mut projections: Vec<Box<dyn Projection>>,
) -> Projected {
    let path = cache_path(dir, id);
    let mut start = 0;
    if let Some(cache) = read_cache(&path)
        && cache.upto <= history.len()
        && cache.digest == digest_of(&history[..cache.upto])
    {
        // Every projection must take up its state, or none does: they all read from the same place in the history.
        let loaded: Vec<bool> = projections
            .iter_mut()
            .map(|p| {
                cache.states.get(p.name()).is_some_and(|saved| {
                    saved.version == p.version() && p.load(saved.state.clone())
                })
            })
            .collect();
        if loaded.iter().all(|ok| *ok) {
            start = cache.upto;
        } else {
            projections = registry_like(&projections);
        }
    }
    for message in &history[start..] {
        for projection in &mut projections {
            projection.push(message);
        }
    }
    let cache = Cache {
        upto: history.len(),
        digest: digest_of(history),
        states: projections
            .iter()
            .map(|p| {
                (
                    p.name().to_owned(),
                    Saved {
                        version: p.version(),
                        state: p.save(),
                    },
                )
            })
            .collect(),
    };
    if let Ok(text) = serde_json::to_string(&cache) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    Projected {
        views: projections
            .iter()
            .map(|p| (p.name().to_owned(), p.view()))
            .collect(),
        folded: history.len() - start,
    }
}

/// Fresh copies of the projections in `like`, by name.
fn registry_like(like: &[Box<dyn Projection>]) -> Vec<Box<dyn Projection>> {
    registry()
        .into_iter()
        .filter(|fresh| like.iter().any(|p| p.name() == fresh.name()))
        .collect()
}

/// The standard projections over a saved thread's history, cached beside the thread.
#[must_use]
pub fn project(id: &str, history: &[Message]) -> Option<Projected> {
    let dir = vivianne_ai::paths::viv_agent_dir().ok()?.join("threads");
    std::fs::create_dir_all(&dir).ok()?;
    Some(project_in(&dir, id, history, registry()))
}

/// Forgets a thread's cached projections.
pub fn forget(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(cache_path(dir, id));
}

#[cfg(test)]
mod tests {
    use vivianne_core::Role;

    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-proj-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn history(turns: usize) -> Vec<Message> {
        (0..turns)
            .flat_map(|i| {
                [
                    Message::new(Role::User, format!("question {i}")).at(1_000 * (i as u64) + 100),
                    Message::assistant(format!("answer {i}"), Vec::new())
                        .at(1_000 * (i as u64) + 600),
                ]
            })
            .collect()
    }

    #[test]
    fn a_second_read_folds_only_what_was_added_and_gets_the_same_answer_as_folding_everything() {
        let dir = dir();
        let first = project_in(&dir, "t", &history(2), registry());
        assert_eq!(first.folded, 4);
        let grown = history(3);
        let second = project_in(&dir, "t", &grown, registry());
        assert_eq!(second.folded, 2, "only the new turn");
        let fresh = project_in(&dir, "other", &grown, registry());
        assert_eq!(
            second.views, fresh.views,
            "the same as folding it all from the start"
        );
        assert_eq!(second.views["stats"]["turns"], 3);
        assert_eq!(second.views["outline"].as_array().map(Vec::len), Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_history_that_was_rewritten_or_cut_is_folded_again_from_the_start() {
        let dir = dir();
        project_in(&dir, "t", &history(3), registry());
        let mut edited = history(3);
        edited[1].text = "a different answer".to_owned();
        assert_eq!(
            project_in(&dir, "t", &edited, registry()).folded,
            6,
            "an earlier message changed"
        );
        assert_eq!(
            project_in(&dir, "t", &history(1), registry()).folded,
            2,
            "the history got shorter"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_state_from_another_version_or_a_broken_file_is_discarded_not_trusted() {
        let dir = dir();
        project_in(&dir, "t", &history(2), registry());
        let path = cache_path(&dir, "t");
        let bumped = std::fs::read_to_string(&path)
            .expect("cache")
            .replace("\"version\":1", "\"version\":0");
        std::fs::write(&path, bumped).expect("tampered");
        assert_eq!(
            project_in(&dir, "t", &history(2), registry()).folded,
            4,
            "a version this build did not write"
        );
        std::fs::write(&path, "{ not json").expect("broken");
        let rebuilt = project_in(&dir, "t", &history(2), registry());
        assert_eq!(
            (rebuilt.folded, rebuilt.views["stats"]["turns"].as_u64()),
            (4, Some(2))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_registry_names_every_projection_once() {
        let names: Vec<_> = registry().iter().map(|p| p.name()).collect();
        assert_eq!(names, ["stats", "outline"]);
        let mut each = names.clone();
        each.dedup();
        assert_eq!(each, names);
    }
}
