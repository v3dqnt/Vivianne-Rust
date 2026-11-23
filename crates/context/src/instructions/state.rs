//! Keeping what the model was told in step with the files. The conversation holds the truth of what was shown (the latest
//! change of each scope); this compares it with the disk and says only what differs: a file added, edited, or gone, or now
//! repeating an earlier one. Port of the original `state.ts`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use vivianne_core::Message;
use vivianne_core::context::{ChangeAction, ContextSource, InstructionChange};

use super::config::Resolved;
use super::files::{
    Fs, Probe, ancestor_chain, content_digest, descendant_dirs_between, relative_display,
    trimmed_digest,
};
use super::render::{
    ChangeItem, LoadedFile, USER_GLOBAL_DIRECTORIES, USER_GLOBAL_FILE, candidate_group,
    candidate_scope_key, decode_scope_key, instruction_scope_key, is_user_global_directory,
    render_changes,
};

/// What is known of one scope's file without keeping its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionState {
    pub path: String,
    pub version: String,
    pub digest: String,
    /// Identity of the trimmed content, to drop repeats without re-reading a sibling.
    pub trimmed_digest: String,
}

/// A cache change that goes with one rendered change.
#[derive(Debug, Clone)]
pub struct VersionUpdate {
    pub change: InstructionChange,
    /// A removal because of a repeat keeps its file's metadata.
    pub state: Option<VersionState>,
}

/// A rendered batch of changes and the cache changes it stands for.
#[derive(Debug, Clone)]
pub struct Reconciled {
    pub text: String,
    pub changes: Vec<InstructionChange>,
    pub updates: Vec<VersionUpdate>,
}

/// The latest change of each scope, over a conversation: what the model currently believes.
#[must_use]
pub fn visible_changes(history: &[Message]) -> HashMap<String, InstructionChange> {
    let mut visible = HashMap::new();
    for message in history {
        if let Some(source) = message
            .context
            .as_ref()
            .and_then(ContextSource::instructions)
        {
            for change in &source.changes {
                visible.insert(change.scope.clone(), change.clone());
            }
        }
    }
    visible
}

/// The baseline source of the newest baseline message in the conversation.
#[must_use]
pub fn visible_baseline(
    history: &[Message],
) -> Option<&vivianne_core::context::InstructionsSource> {
    history
        .iter()
        .rev()
        .filter_map(|m| m.context.as_ref().and_then(ContextSource::instructions))
        .find(|s| s.baseline)
}

/// The scope states and cache states of a rendered baseline's files.
#[must_use]
pub fn baseline_state(
    files: &[LoadedFile],
) -> (
    BTreeMap<String, InstructionChange>,
    HashMap<String, VersionState>,
) {
    let mut changes = BTreeMap::new();
    let mut versions = HashMap::new();
    for file in files {
        let digest = content_digest(&file.content);
        let scope = instruction_scope_key(&file.display);
        changes.insert(
            scope.clone(),
            InstructionChange {
                action: ChangeAction::Set,
                scope: scope.clone(),
                path: file.display.clone(),
                digest: Some(digest.clone()),
            },
        );
        if let Some(version) = &file.version {
            versions.insert(
                scope,
                VersionState {
                    path: file.display.clone(),
                    version: version.clone(),
                    digest,
                    trimmed_digest: trimmed_digest(&file.content),
                },
            );
        }
    }
    (changes, versions)
}

/// Where a scope's file is and how the model sees its path.
struct ScopeFile {
    absolute: PathBuf,
    display: String,
}

fn scope_file(scope: &str, root: &Path, resolved: &Resolved) -> ScopeFile {
    let (directory, candidate) = decode_scope_key(scope);
    let global = if directory == USER_GLOBAL_DIRECTORIES[0] {
        Some((&resolved.viv_home, &resolved.viv_home_display))
    } else if directory == USER_GLOBAL_DIRECTORIES[1] {
        Some((&resolved.agents_home, &resolved.agents_home_display))
    } else {
        None
    };
    let dir = match global {
        Some((home, _)) => home.clone(),
        None if directory == "." => root.to_path_buf(),
        None => root.join(&directory),
    };
    let absolute = dir.join(&candidate);
    let display = match global {
        Some((_, shown)) => format!("{shown}/{candidate}"),
        None => relative_display(root, &absolute),
    };
    ScopeFile { absolute, display }
}

fn relative_scope(root: &Path, dir: &Path) -> String {
    let scope = relative_display(root, dir);
    if scope.is_empty() {
        ".".to_owned()
    } else {
        scope
    }
}

/// What a pass looks at and how it treats the baseline.
#[derive(Debug)]
pub struct Pass<'a> {
    pub cwd: &'a Path,
    pub project_root: &'a Path,
    /// The newest change of each scope as the model currently believes it, including any baseline composed in this pass.
    pub effective: &'a HashMap<String, InstructionChange>,
    /// Paths a tool just read, wrote or edited: folders they crossed may hold instructions of their own.
    pub touched_paths: &'a [String],
    /// Whether the baseline's own scopes are probed too (a baseline is already visible).
    pub include_baseline_scopes: bool,
    /// Baseline scopes the baseline left out because of the budget or a repeat: removed from the model's view if shown.
    pub excluded_baseline_scopes: Option<&'a HashSet<String>>,
}

fn push_unique(list: &mut Vec<String>, scope: String) {
    if !list.contains(&scope) {
        list.push(scope);
    }
}

/// Compares the visible state with the disk and renders what changed. `None` when nothing did, when a probe failed, or when
/// nothing that changed fit the budget (the cache is then left alone, so the next pass tries again).
pub fn reconcile(
    resolved: &Resolved,
    versions: &mut HashMap<String, VersionState>,
    fs: &dyn Fs,
    pass: &Pass<'_>,
) -> Option<Reconciled> {
    let root = pass.project_root;
    let candidates: Vec<&String> = resolved
        .config
        .instruction_file_candidates
        .iter()
        .chain(&resolved.config.local_instruction_file_candidates)
        .collect();
    let dir_scopes = |directory: &str| -> Vec<String> {
        candidates
            .iter()
            .map(|c| candidate_scope_key(directory, c))
            .collect()
    };

    let mut scopes: Vec<String> = Vec::new();
    let mut baseline_scopes: HashSet<String> = HashSet::new();
    for directory in USER_GLOBAL_DIRECTORIES {
        baseline_scopes.insert(candidate_scope_key(directory, USER_GLOBAL_FILE));
    }
    for dir in ancestor_chain(root, pass.cwd) {
        baseline_scopes.extend(dir_scopes(&relative_scope(root, &dir)));
    }
    if pass.include_baseline_scopes {
        let mut ordered: Vec<String> = Vec::new();
        for directory in USER_GLOBAL_DIRECTORIES {
            ordered.push(candidate_scope_key(directory, USER_GLOBAL_FILE));
        }
        for dir in ancestor_chain(root, pass.cwd) {
            ordered.extend(dir_scopes(&relative_scope(root, &dir)));
        }
        for scope in ordered {
            push_unique(&mut scopes, scope);
        }
    }
    let mut effective_scopes: Vec<&String> = pass.effective.keys().collect();
    effective_scopes.sort();
    for scope in effective_scopes {
        if !pass.include_baseline_scopes && baseline_scopes.contains(scope) {
            continue;
        }
        let (directory, _) = decode_scope_key(scope);
        if is_user_global_directory(&directory) {
            push_unique(
                &mut scopes,
                candidate_scope_key(&directory, USER_GLOBAL_FILE),
            );
        } else {
            for s in dir_scopes(&directory) {
                push_unique(&mut scopes, s);
            }
        }
    }
    for touched in pass.touched_paths {
        for dir in descendant_dirs_between(pass.cwd, touched) {
            for s in dir_scopes(&relative_scope(root, &dir)) {
                push_unique(&mut scopes, s);
            }
        }
    }

    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut items: Vec<ChangeItem> = Vec::new();
    let mut updates: Vec<VersionUpdate> = Vec::new();
    // Scopes in groups, keeping first-seen order: the two user-global roots share one.
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for scope in scopes {
        let (directory, _) = decode_scope_key(&scope);
        let group = candidate_group(&directory);
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, list)) => list.push(scope),
            None => groups.push((group, vec![scope])),
        }
    }

    for (_, group_scopes) in groups {
        let mut group_items: Vec<ChangeItem> = Vec::new();
        let mut group_updates: Vec<VersionUpdate> = Vec::new();
        let mut group_versions: Vec<(String, Option<VersionState>)> = Vec::new();
        let mut kept_trimmed: HashSet<String> = HashSet::new();
        let mut probed: Vec<String> = Vec::new();
        for scope in &group_scopes {
            if pass
                .excluded_baseline_scopes
                .is_some_and(|ex| ex.contains(scope))
                && baseline_scopes.contains(scope)
            {
                match pass.effective.get(scope) {
                    Some(prev) if prev.action != ChangeAction::Remove => push_removal(
                        &mut group_items,
                        &mut group_updates,
                        scope,
                        &prev.path,
                        None,
                    ),
                    _ => group_versions.push((scope.clone(), None)),
                }
            } else {
                probed.push(scope.clone());
            }
        }
        let mut failed = false;
        let mut group_seen: Vec<PathBuf> = Vec::new();
        for scope in &probed {
            let previous = pass.effective.get(scope);
            let file = scope_file(scope, root, resolved);
            let (version, _size) = match fs.probe(&file.absolute) {
                Probe::Unavailable => {
                    failed = true;
                    break;
                }
                Probe::Absent => {
                    match previous {
                        Some(prev) if prev.action != ChangeAction::Remove => push_removal(
                            &mut group_items,
                            &mut group_updates,
                            scope,
                            &prev.path,
                            None,
                        ),
                        _ => group_versions.push((scope.clone(), None)),
                    }
                    continue;
                }
                Probe::Present { version, size } => (version, size),
            };
            if seen_paths.contains(&file.absolute) || group_seen.contains(&file.absolute) {
                continue;
            }
            group_seen.push(file.absolute.clone());
            let was_rendered = |cached: &VersionState| {
                previous.is_some_and(|p| {
                    p.action != ChangeAction::Remove
                        && p.path == cached.path
                        && p.digest.as_deref() == Some(cached.digest.as_str())
                })
            };
            let hidden = previous.is_none_or(|p| p.action == ChangeAction::Remove);
            if let Some(cached) = versions
                .get(scope)
                .filter(|c| c.path == file.display && c.version == version)
            {
                if was_rendered(cached) {
                    // Unchanged and shown: keep it, unless an earlier member of the group now says the same.
                    if !kept_trimmed.insert(cached.trimmed_digest.clone()) {
                        push_removal(
                            &mut group_items,
                            &mut group_updates,
                            scope,
                            &cached.path,
                            Some(cached.clone()),
                        );
                    }
                    continue;
                }
                // A hidden duplicate stays hidden: only a promotion needs another read.
                if hidden && kept_trimmed.contains(&cached.trimmed_digest) {
                    continue;
                }
            }
            let Some(content) = fs.read_text(&file.absolute, resolved.config.max_source_bytes)
            else {
                failed = true;
                break;
            };
            let digest = content_digest(&content);
            let trimmed = trimmed_digest(&content);
            let next = VersionState {
                path: file.display.clone(),
                version: version.clone(),
                digest: digest.clone(),
                trimmed_digest: trimmed.clone(),
            };
            if !kept_trimmed.insert(trimmed) {
                // A distinct file whose trimmed text already appeared earlier in the group: drop it (and any copy shown
                // before), keeping its metadata so the next pass can decide without reading it again.
                match previous {
                    Some(prev) if prev.action != ChangeAction::Remove => {
                        push_removal(
                            &mut group_items,
                            &mut group_updates,
                            scope,
                            &prev.path,
                            Some(next),
                        );
                    }
                    _ => group_versions.push((scope.clone(), Some(next))),
                }
                continue;
            }
            if previous.is_some_and(|p| {
                p.action != ChangeAction::Remove
                    && p.path == file.display
                    && p.digest.as_deref() == Some(digest.as_str())
            }) {
                group_versions.push((scope.clone(), Some(next)));
                continue;
            }
            let action = if hidden {
                ChangeAction::Set
            } else {
                ChangeAction::Replace
            };
            let change = InstructionChange {
                action,
                scope: scope.clone(),
                path: file.display.clone(),
                digest: Some(digest),
            };
            group_items.push(ChangeItem {
                change: change.clone(),
                file: LoadedFile {
                    absolute: file.absolute.display().to_string(),
                    display: file.display.clone(),
                    content,
                    version: Some(version),
                },
            });
            group_updates.push(VersionUpdate {
                change,
                state: Some(next),
            });
        }
        if failed {
            // Metadata and content availability apply to the whole group, hidden candidates included: a failed group
            // publishes nothing, and its paths are never reached by an alias.
            for scope in &probed {
                seen_paths.insert(scope_file(scope, root, resolved).absolute);
            }
            continue;
        }
        seen_paths.extend(group_seen);
        for (scope, state) in group_versions {
            match state {
                Some(s) => versions.insert(scope, s),
                None => versions.remove(&scope),
            };
        }
        items.extend(group_items);
        updates.extend(group_updates);
    }
    if items.is_empty() {
        return None;
    }
    let (text, changes) = render_changes(&items, resolved.config.max_bytes);
    // Nothing fit (a tiny budget renders only a notice): say nothing and keep the cache as it was, so the next pass retries
    // instead of repeating a notice.
    if text.is_empty() || changes.is_empty() {
        return None;
    }
    let updates = updates
        .into_iter()
        .filter(|u| changes.contains(&u.change))
        .collect();
    Some(Reconciled {
        text,
        changes,
        updates,
    })
}

fn push_removal(
    items: &mut Vec<ChangeItem>,
    updates: &mut Vec<VersionUpdate>,
    scope: &str,
    path: &str,
    state: Option<VersionState>,
) {
    let change = InstructionChange {
        action: ChangeAction::Remove,
        scope: scope.to_owned(),
        path: path.to_owned(),
        digest: None,
    };
    items.push(ChangeItem {
        change: change.clone(),
        file: LoadedFile {
            absolute: format!("removed:{scope}"),
            display: path.to_owned(),
            content: String::new(),
            version: None,
        },
    });
    updates.push(VersionUpdate { change, state });
}

/// Applies the cache changes of the rendered batch.
pub fn apply_updates(versions: &mut HashMap<String, VersionState>, updates: &[VersionUpdate]) {
    for update in updates {
        match &update.state {
            Some(s) => versions.insert(update.change.scope.clone(), s.clone()),
            None => versions.remove(&update.change.scope),
        };
    }
}
