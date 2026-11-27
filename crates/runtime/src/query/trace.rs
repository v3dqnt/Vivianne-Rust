//! Relationships: which conversation a branch came from and which came from it, and which events of one conversation
//! answer or cite which others. Pure over the records it is given.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use vivianne_core::{Message, Role};

use super::ThreadRecord;

/// The deepest a branch tree is followed, and the most nodes returned: a conversation cannot be made to walk forever.
pub const MAX_DEPTH: usize = 32;
pub const MAX_NODES: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineageNode {
    pub thread: ThreadRecord,
    pub descendants: Vec<LineageNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lineage {
    pub target: ThreadRecord,
    /// The parent first, then its parent, up to the root.
    pub ancestors: Vec<ThreadRecord>,
    pub descendants: Vec<LineageNode>,
    /// False when an ancestor's thread no longer exists; `unresolved_parent` says which.
    pub complete: bool,
    pub unresolved_parent: Option<String>,
    /// The tree was cut at `MAX_DEPTH` or `MAX_NODES`.
    pub truncated: bool,
}

/// The lineage of `target` among `all`, which must hold every thread.
#[must_use]
pub fn lineage(target: &ThreadRecord, all: &[ThreadRecord]) -> Lineage {
    let by_id: BTreeMap<&str, &ThreadRecord> = all.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut ancestors = Vec::new();
    let mut seen = BTreeSet::from([target.id.as_str()]);
    let mut next = target.parent.as_ref().map(|p| p.thread.as_str());
    let mut unresolved = None;
    while let Some(id) = next {
        // A parent that points back down the chain is a loop, not a root.
        if !seen.insert(id) {
            break;
        }
        match by_id.get(id) {
            Some(parent) => {
                ancestors.push((*parent).clone());
                next = parent.parent.as_ref().map(|p| p.thread.as_str());
            }
            None => {
                unresolved = Some(id.to_owned());
                break;
            }
        }
    }
    let mut children: BTreeMap<&str, Vec<&ThreadRecord>> = BTreeMap::new();
    for thread in all {
        if let Some(parent) = &thread.parent {
            children
                .entry(parent.thread.as_str())
                .or_default()
                .push(thread);
        }
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| (a.updated_at, &a.id).cmp(&(b.updated_at, &b.id)));
    }
    let mut budget = MAX_NODES;
    let mut truncated = false;
    let mut visited = BTreeSet::from([target.id.clone()]);
    let descendants = grow(
        &target.id,
        &children,
        1,
        &mut budget,
        &mut truncated,
        &mut visited,
    );
    Lineage {
        target: target.clone(),
        ancestors,
        descendants,
        complete: unresolved.is_none(),
        unresolved_parent: unresolved,
        truncated,
    }
}

fn grow(
    id: &str,
    children: &BTreeMap<&str, Vec<&ThreadRecord>>,
    depth: usize,
    budget: &mut usize,
    truncated: &mut bool,
    visited: &mut BTreeSet<String>,
) -> Vec<LineageNode> {
    let mut out = Vec::new();
    for child in children.get(id).into_iter().flatten() {
        if !visited.insert(child.id.clone()) {
            continue;
        }
        if *budget == 0 || depth > MAX_DEPTH {
            *truncated = true;
            break;
        }
        *budget -= 1;
        out.push(LineageNode {
            thread: (*child).clone(),
            descendants: grow(&child.id, children, depth + 1, budget, truncated, visited),
        });
    }
    out
}

/// How one event relates to others in the same conversation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct EventTrace {
    /// Events this one answers: a tool result is the answer to a call a reply made.
    pub source_seqs: Vec<usize>,
    /// Events that answer this one: the results of the calls a reply made.
    pub derived_seqs: Vec<usize>,
}

/// The relationships of the event at `seq` in `history`; `None` when there is no such event.
#[must_use]
pub fn event_trace(history: &[Message], seq: usize) -> Option<EventTrace> {
    let message = history.get(seq)?;
    let mut trace = EventTrace::default();
    match message.role {
        Role::Tool => {
            if let Some(id) = &message.tool_call_id {
                trace.source_seqs = history
                    .iter()
                    .enumerate()
                    .take(seq)
                    .filter(|(_, m)| m.tool_calls.iter().any(|c| c.id == *id))
                    .map(|(i, _)| i)
                    .collect();
            }
        }
        Role::Assistant if !message.tool_calls.is_empty() => {
            let ids: BTreeSet<&str> = message.tool_calls.iter().map(|c| c.id.as_str()).collect();
            trace.derived_seqs = history
                .iter()
                .enumerate()
                .skip(seq + 1)
                .filter(|(_, m)| {
                    m.role == Role::Tool
                        && m.tool_call_id.as_deref().is_some_and(|id| ids.contains(id))
                })
                .map(|(i, _)| i)
                .collect();
        }
        _ => {}
    }
    Some(trace)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;

    use super::*;
    use crate::store::Parent;

    fn record(id: &str, parent: Option<&str>, at: u64) -> ThreadRecord {
        ThreadRecord {
            id: id.into(),
            workspace: "/w".into(),
            title: id.into(),
            updated_at: at,
            parent: parent.map(|p| Parent {
                thread: p.into(),
                turn: 1,
            }),
            messages: 0,
            generations: vec![],
        }
    }

    fn ids(nodes: &[LineageNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.thread.id.as_str()).collect()
    }

    #[test]
    fn ancestors_run_to_the_root_and_descendants_form_a_tree_oldest_first() {
        let all = [
            record("root", None, 1),
            record("mid", Some("root"), 2),
            record("leaf", Some("mid"), 3),
            record("twin", Some("mid"), 2),
            record("other", None, 9),
        ];
        let got = lineage(&all[1], &all);
        assert_eq!(
            got.ancestors
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["root"]
        );
        assert_eq!(ids(&got.descendants), ["twin", "leaf"]);
        assert!(got.complete && !got.truncated);
        let top = lineage(&all[0], &all);
        assert_eq!(ids(&top.descendants), ["mid"]);
        assert_eq!(ids(&top.descendants[0].descendants), ["twin", "leaf"]);
    }

    #[test]
    fn a_missing_ancestor_makes_the_lineage_incomplete_and_names_it() {
        let all = [record("leaf", Some("gone"), 1)];
        let got = lineage(&all[0], &all);
        assert_eq!(
            (got.complete, got.unresolved_parent.as_deref()),
            (false, Some("gone"))
        );
        assert!(got.ancestors.is_empty());
    }

    #[test]
    fn a_loop_of_parents_ends_instead_of_walking_forever() {
        let all = [record("a", Some("b"), 1), record("b", Some("a"), 2)];
        let got = lineage(&all[0], &all);
        assert_eq!(got.ancestors.len(), 1);
        assert!(got.descendants.is_empty() || got.descendants.len() == 1);
    }

    #[test]
    fn a_result_names_the_reply_that_asked_and_a_reply_names_its_results() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "read".into(),
            args: json!({}),
        };
        let history = vec![
            Message::new(Role::User, "go"),
            Message::assistant(String::new(), vec![call("a"), call("b")]),
            Message::tool_result("a", "x".to_owned()),
            Message::tool_result("b", "y".to_owned()),
        ];
        assert_eq!(
            event_trace(&history, 2).map(|t| t.source_seqs),
            Some(vec![1])
        );
        assert_eq!(
            event_trace(&history, 1).map(|t| t.derived_seqs),
            Some(vec![2, 3])
        );
        assert_eq!(event_trace(&history, 0), Some(EventTrace::default()));
        assert_eq!(event_trace(&history, 9), None);
    }
}
