//! The instruction loader of one conversation. Each step it looks at what the conversation already shows, decides whether
//! a baseline (the complete set) or only changes are due, and returns the one durable message to add, or nothing. Port of
//! the original plugin's `compose`, without its inbox: here the caller owns the history and adds the message itself.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use vivianne_core::Message;
use vivianne_core::context::{ChangeAction, ContextSource, InstructionChange, InstructionsSource};

use super::config::Resolved;
use super::files::{Fs, find_project_root, load_baseline, trimmed_digest};
use super::render::{candidate_group, instruction_scope_key, scope_for_display_path};
use super::state::{
    Pass, VersionState, apply_updates, baseline_state, reconcile, visible_baseline, visible_changes,
};

/// What was prepared for the baseline now showing: the rules it was made under, and the scopes it left out.
#[derive(Debug, Default)]
struct Prepared {
    identity: String,
    excluded: HashSet<String>,
}

#[derive(Debug, Default)]
struct State {
    versions: HashMap<String, VersionState>,
    prepared: Option<Prepared>,
}

/// One conversation's loader. State is metadata only: instruction text is never kept.
#[derive(Debug)]
pub struct Engine {
    resolved: Resolved,
    state: Mutex<State>,
}

impl Engine {
    #[must_use]
    pub fn new(resolved: Resolved) -> Self {
        Self {
            resolved,
            state: Mutex::new(State::default()),
        }
    }

    #[must_use]
    pub fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    /// The message to add before the next model call, if any. `touched` are the paths tools read, wrote or edited since the
    /// last call: a folder they reach may hold instructions of its own. An error is a failed root probe (permission or I/O);
    /// another project's files are never substituted for it.
    pub fn compose(
        &self,
        history: &[Message],
        cwd: &Path,
        touched: &[String],
        fs: &dyn Fs,
    ) -> std::io::Result<Option<Message>> {
        if !self.resolved.enabled() {
            return Ok(None);
        }
        let project_root = find_project_root(cwd, &self.resolved.config.project_root_markers, fs)?;
        let identity = self.resolved.baseline_identity(cwd, &project_root);
        let shown = visible_baseline(history);
        let baseline_present = shown.is_some();
        let keep_visible =
            shown.is_some_and(|b| b.baseline_identity.as_deref() == Some(identity.as_str()));
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut excluded: Option<HashSet<String>> = match &state.prepared {
            Some(p) if keep_visible && p.identity == identity => Some(p.excluded.clone()),
            _ => None,
        };
        let mut effective = visible_changes(history);
        let mut texts: Vec<String> = Vec::new();
        let mut changes: Vec<InstructionChange> = Vec::new();
        let mut desired_baseline = false;
        let mut next_prepared: Option<Prepared> = None;

        if !baseline_present || !keep_visible || excluded.is_none() {
            let replace_previous = baseline_present && !keep_visible;
            let set = load_baseline(&self.resolved, cwd, &project_root, replace_previous, fs);
            let included = set
                .as_ref()
                .map(|s| s.included.as_slice())
                .unwrap_or_default();
            let observed = set
                .as_ref()
                .map(|s| s.observed.as_slice())
                .unwrap_or_default();
            let (baseline_changes, baseline_versions) = baseline_state(included);
            let (observed_changes, _) = baseline_state(observed);
            let mut included_digests: HashMap<String, HashSet<String>> = HashMap::new();
            for file in included {
                included_digests
                    .entry(candidate_group(&scope_for_display_path(&file.display)))
                    .or_default()
                    .insert(trimmed_digest(&file.content));
            }
            // A duplicate inherits its winner's omission; only duplicates of shown content stay eligible to be promoted.
            let mut excluded_scopes: HashSet<String> = observed_changes.keys().cloned().collect();
            for scope in baseline_changes.keys() {
                excluded_scopes.remove(scope);
            }
            for file in set
                .as_ref()
                .map(|s| s.deduped.as_slice())
                .unwrap_or_default()
            {
                let group = candidate_group(&scope_for_display_path(&file.display));
                if included_digests
                    .get(&group)
                    .is_some_and(|d| d.contains(&trimmed_digest(&file.content)))
                {
                    excluded_scopes.remove(&instruction_scope_key(&file.display));
                }
            }
            for (scope, version) in baseline_versions {
                state.versions.insert(scope, version);
            }
            if !keep_visible && let Some(set) = set.as_ref().filter(|s| !s.rendered.text.is_empty())
            {
                {
                    texts.push(set.rendered.text.clone());
                    let replacement_removals: Vec<InstructionChange> = if replace_previous {
                        shown
                            .map(|b| b.changes.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .filter(|c| {
                                c.action != ChangeAction::Remove
                                    && !baseline_changes.contains_key(&c.scope)
                            })
                            .map(|c| InstructionChange {
                                action: ChangeAction::Remove,
                                scope: c.scope.clone(),
                                path: c.path.clone(),
                                digest: None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let all: Vec<InstructionChange> = replacement_removals
                        .into_iter()
                        .chain(baseline_changes.values().cloned())
                        .collect();
                    // What this very message announces is what the pass below compares with.
                    for change in &all {
                        effective.insert(change.scope.clone(), change.clone());
                    }
                    changes.extend(all);
                    desired_baseline = true;
                }
            }
            excluded = Some(excluded_scopes.clone());
            next_prepared = Some(Prepared {
                identity: identity.clone(),
                excluded: excluded_scopes,
            });
        }

        let pass = Pass {
            cwd,
            project_root: &project_root,
            effective: &effective,
            touched_paths: touched,
            include_baseline_scopes: keep_visible,
            excluded_baseline_scopes: if keep_visible {
                excluded.as_ref()
            } else {
                None
            },
        };
        if let Some(update) = reconcile(&self.resolved, &mut state.versions, fs, &pass) {
            texts.push(update.text.clone());
            changes.extend(update.changes.iter().cloned());
            apply_updates(&mut state.versions, &update.updates);
        }
        if let Some(prepared) = next_prepared {
            state.prepared = Some(prepared);
        }
        if texts.is_empty() {
            return Ok(None);
        }
        let source = InstructionsSource {
            baseline: desired_baseline,
            baseline_identity: desired_baseline.then_some(identity),
            changes,
        };
        Ok(Some(Message::context(
            texts.join("\n\n"),
            ContextSource::AgentInstructions(source),
        )))
    }
}
