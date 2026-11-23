//! Keeps the conversation fit to send: every tool call answered, every answer attached to a call, and old bulky
//! tool output shrunk. Pure functions on the message list.

use std::collections::HashMap;

use vivianne_core::{Message, Role};

use crate::context::message_tokens;

/// What a call that has no recorded result is answered with, so the provider accepts the history.
const UNKNOWN_RESULT: &str = "error: no result was recorded for this call (the run was interrupted); its outcome is unknown, so check before assuming it did or did not happen";

/// The newest tool results kept in full.
const KEEP_RECENT_RESULTS: usize = 10;
/// Results shorter than this stay: they are cheap and often the whole point ("tests passed").
const MASK_MIN_CHARS: usize = 1_200;
/// Old results are shrunk in one batch once this much has built up, not a little every step: each change to an
/// old message makes every provider's prompt cache start over from that message.
const MASK_BATCH_CHARS: usize = 80_000;
const MASK_MARKER: &str = "[Earlier output elided";

/// Makes the history acceptable to a provider: each assistant message's calls are answered right after it (a
/// missing result becomes an error saying the outcome is unknown), and a result with no call is dropped. A run
/// that was cut off (the app closed, a crash) leaves such holes, and a provider refuses the whole history for them.
pub fn repair(history: &mut Vec<Message>) {
    let mut out = Vec::with_capacity(history.len());
    let mut messages = std::mem::take(history).into_iter().peekable();
    while let Some(message) = messages.next() {
        match message.role {
            Role::Tool => {} // an orphan: its call was not directly before it
            Role::Assistant if !message.tool_calls.is_empty() => {
                let mut answers = HashMap::new();
                while let Some(next) = messages.next_if(|m| m.role == Role::Tool) {
                    if let Some(id) = next.tool_call_id.clone() {
                        answers.entry(id).or_insert(next);
                    }
                }
                let calls = message.tool_calls.clone();
                out.push(message);
                for call in calls {
                    out.push(
                        answers
                            .remove(&call.id)
                            .unwrap_or_else(|| Message::tool_result(&call.id, UNKNOWN_RESULT)),
                    );
                }
            }
            _ => out.push(message),
        }
    }
    *history = out;
}

/// Shrinks old, large tool results to a note saying what they were (a failure keeps its error lines). Does
/// nothing until enough has built up to be worth one cache rewrite. `true` if it changed anything.
pub fn mask_old_results(history: &mut [Message]) -> bool {
    // The desktop's snapshots are replaced as soon as a newer one exists, not when enough has piled up.
    let snapshots = mask_stale_snapshots(history);
    mask_by_size(history) || snapshots
}

/// The name of the tool that the call `id` made.
fn name_of(before: &[Message], id: Option<&str>) -> Option<String> {
    before
        .iter()
        .rev()
        .flat_map(|m| m.tool_calls.iter())
        .find(|c| Some(c.id.as_str()) == id)
        .map(|c| c.name.clone())
}

/// The tools whose result is the state of a screen or page at one moment.
const SNAPSHOT_TOOLS: [&str; 2] = [
    "mcp__computer__desktop_snapshot",
    "mcp__browser__agent_browser_snapshot",
];

/// A snapshot of an app is the state of its screen at one moment: once the agent has taken a newer one, an older one
/// only costs context (and is a picture of what was private). Every snapshot but the newest becomes a note.
fn mask_stale_snapshots(history: &mut [Message]) -> bool {
    let snapshots: Vec<usize> = (0..history.len())
        .filter(|&i| {
            history[i].role == Role::Tool
                && name_of(&history[..i], history[i].tool_call_id.as_deref())
                    .is_some_and(|n| SNAPSHOT_TOOLS.contains(&n.as_str()))
        })
        .collect();
    let mut changed = false;
    for &i in snapshots.iter().rev().skip(1) {
        if history[i].text.starts_with(MASK_MARKER) || history[i].text.chars().count() < 300 {
            continue;
        }
        history[i].text = format!(
            "{MASK_MARKER} to save context: a snapshot of an app, replaced by a newer one.]"
        );
        changed = true;
    }
    changed
}

fn mask_by_size(history: &mut [Message]) -> bool {
    let results: Vec<usize> = (0..history.len())
        .filter(|&i| history[i].role == Role::Tool)
        .collect();
    let Some(old) = results
        .len()
        .checked_sub(KEEP_RECENT_RESULTS)
        .map(|n| &results[..n])
    else {
        return false;
    };
    let eligible: Vec<usize> = old
        .iter()
        .copied()
        .filter(|&i| {
            !history[i].text.starts_with(MASK_MARKER)
                && history[i].text.chars().count() > MASK_MIN_CHARS
        })
        .collect();
    let pending: usize = eligible.iter().map(|&i| history[i].text.len()).sum();
    if pending < MASK_BATCH_CHARS {
        return false;
    }
    for i in eligible {
        let what = call_of(&history[..i], history[i].tool_call_id.as_deref())
            .unwrap_or_else(|| "a tool call".to_owned());
        let mut note = format!(
            "{MASK_MARKER} to save context: {what} ({} KB). Run it again if you need it.]",
            history[i].text.len() / 1024 + 1
        );
        if history[i].text.starts_with("error: ") {
            note.push_str("\nIt failed:\n");
            note.push_str(&error_lines(&history[i].text));
        }
        history[i].text = note;
    }
    true
}

/// What stands in a saved thread for what the desktop showed.
pub const DESKTOP_NOT_KEPT: &str = "[what the desktop showed here is not kept]";

/// The history as it may be kept: what the desktop tools returned (what an app showed, which windows were open, the
/// clipboard) is replaced by a note. It was private to the moment; saving it would put it on disk, in memory and in
/// anything that reads threads.
#[must_use]
pub fn without_desktop_content(history: &[Message]) -> Vec<Message> {
    history
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let private = m.role == Role::Tool
                && name_of(&history[..i], m.tool_call_id.as_deref())
                    .is_some_and(|n| vivianne_core::returns_desktop_content(&n));
            if private {
                Message {
                    text: DESKTOP_NOT_KEPT.to_owned(),
                    ..m.clone()
                }
            } else {
                m.clone()
            }
        })
        .collect()
}

/// `name arg` for the call that `id` answers, e.g. `read src/main.rs`.
fn call_of(before: &[Message], id: Option<&str>) -> Option<String> {
    let call = before
        .iter()
        .rev()
        .flat_map(|m| m.tool_calls.iter())
        .find(|c| Some(c.id.as_str()) == id)?;
    let arg = ["path", "command", "pattern", "query"]
        .iter()
        .find_map(|k| call.args[k].as_str())?;
    Some(format!(
        "{} {}",
        call.name,
        arg.chars().take(120).collect::<String>()
    ))
}

/// The lines of a failure that say what failed: the first few and the last few, under about 800 characters.
fn error_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let picked = if lines.len() <= 10 {
        lines
    } else {
        [&lines[..5], &["…"], &lines[lines.len() - 4..]].concat()
    };
    picked.join("\n").chars().take(800).collect()
}

/// Whether the history may be cut before message `at` without separating a call from its result.
fn is_balanced_cut(history: &[Message], at: usize) -> bool {
    let mut open = 0usize;
    for message in &history[..at] {
        match message.role {
            Role::Assistant => open += message.tool_calls.len(),
            Role::Tool => open = open.saturating_sub(1),
            _ => {}
        }
    }
    open == 0 && history.get(at).is_some_and(|m| m.role != Role::Tool)
}

/// Where to cut so about `keep_tokens` of the newest conversation survive: the balanced cut nearest the point
/// where that many tokens are kept, preferring to keep more. 0 when there is too little to be worth condensing.
#[must_use]
pub fn pick_cut(history: &[Message], keep_tokens: u32, min_messages: usize) -> usize {
    if history.len() < min_messages {
        return 0;
    }
    let mut kept = 0u32;
    let mut target = 0;
    for (i, message) in history.iter().enumerate().rev() {
        kept = kept.saturating_add(message_tokens(message));
        if kept > keep_tokens {
            target = i;
            break;
        }
    }
    if target == 0 {
        return 0;
    }
    let ok = |i: usize| i >= 1 && is_balanced_cut(history, i);
    let before = (1..=target).rev().find(|&i| ok(i));
    let after = (target..history.len()).find(|&i| ok(i));
    match (before, after) {
        (Some(b), Some(a)) => {
            if target - b <= a - target {
                b
            } else {
                a
            }
        }
        (b, a) => b.or(a).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;

    use super::*;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "read".into(),
            args: json!({"path": "a.rs"}),
        }
    }

    fn big(id: &str, n: usize) -> [Message; 2] {
        [
            Message::assistant("", vec![call(id)]),
            Message::tool_result(id, "x".repeat(n)),
        ]
    }

    #[test]
    fn repair_answers_missing_results_and_drops_orphans() {
        let mut history = vec![
            Message::new(Role::User, "go"),
            Message::assistant("", vec![call("a"), call("b")]),
            Message::tool_result("b", "got b"),
            Message::tool_result("zzz", "an orphan"),
            Message::new(Role::User, "next"),
        ];
        repair(&mut history);
        let shape: Vec<_> = history
            .iter()
            .map(|m| (m.tool_call_id.as_deref(), m.text.as_str()))
            .collect();
        assert_eq!(shape.len(), 5);
        assert_eq!(
            (shape[2].0, shape[3].0),
            (Some("a"), Some("b")),
            "answers follow the call, in its order"
        );
        assert!(shape[2].1.contains("unknown") && shape[3].1 == "got b" && shape[4].1 == "next");
        let again = history.clone();
        repair(&mut history);
        assert_eq!(
            history, again,
            "repairing a repaired history changes nothing"
        );
    }

    #[test]
    fn old_output_is_masked_in_one_batch_and_the_newest_stay() {
        let mut history = vec![Message::new(Role::User, "go")];
        for i in 0..20 {
            history.extend(big(&format!("c{i}"), 20_000));
        }
        assert!(mask_old_results(&mut history));
        let masked = history
            .iter()
            .filter(|m| m.text.starts_with(MASK_MARKER))
            .count();
        assert_eq!(masked, 10, "all but the newest ten");
        assert!(history[2].text.contains("read a.rs"));
        assert!(!mask_old_results(&mut history), "nothing left to do");
        let mut few = vec![Message::new(Role::User, "go")];
        for i in 0..20 {
            few.extend(big(&format!("c{i}"), 2_000));
        }
        assert!(
            !mask_old_results(&mut few),
            "not enough built up to be worth rewriting the cache"
        );
    }

    #[test]
    fn a_cut_never_separates_a_call_from_its_result() {
        let mut history = vec![Message::new(Role::User, "one")];
        for i in 0..6 {
            history.extend(big(&format!("c{i}"), 400));
        }
        history.push(Message::new(Role::User, "last"));
        for keep in [1, 100, 300, 600] {
            let cut = pick_cut(&history, keep, 4);
            assert!(cut == 0 || is_balanced_cut(&history, cut), "{keep}: {cut}");
        }
        assert_ne!(pick_cut(&history, 300, 4), 0);
        assert_eq!(pick_cut(&history, 300, 100), 0, "too short to be worth it");
    }

    fn desktop_call(id: &str, tool: &str) -> Message {
        Message::assistant(
            "",
            vec![vivianne_core::ToolCall {
                id: id.into(),
                name: tool.into(),
                args: serde_json::json!({}),
            }],
        )
    }

    fn result(id: &str, text: &str) -> Message {
        Message {
            role: Role::Tool,
            text: text.into(),
            tool_calls: vec![],
            tool_call_id: Some(id.into()),
            attachments: vec![],
        }
    }

    #[test]
    fn only_the_newest_snapshot_is_kept_in_full() {
        let big = "x".repeat(2_000);
        let mut history = vec![
            Message::new(Role::User, "go"),
            desktop_call("1", SNAPSHOT_TOOLS[0]),
            result("1", &big),
            desktop_call("2", "mcp__computer__desktop_click"),
            result("2", &big),
            desktop_call("3", SNAPSHOT_TOOLS[0]),
            result("3", &big),
        ];
        assert!(mask_old_results(&mut history));
        assert!(
            history[2].text.starts_with(MASK_MARKER),
            "the older snapshot becomes a note"
        );
        assert_eq!(history[4].text, big, "other results wait for the size rule");
        assert_eq!(history[6].text, big, "the newest snapshot stays");
        assert!(!mask_old_results(&mut history), "nothing more to do");
    }

    #[test]
    fn what_the_desktop_showed_is_not_in_what_is_kept_but_other_results_are() {
        let history = vec![
            Message::new(Role::User, "look"),
            desktop_call("1", SNAPSHOT_TOOLS[0]),
            result("1", "Notepad: meow (unsaved)"),
            desktop_call("2", "read"),
            result("2", "fn main() {}"),
            desktop_call("3", "mcp__computer__desktop_screenshot"),
            result("3", "base64..."),
        ];
        let kept = without_desktop_content(&history);
        assert_eq!(
            (
                kept[2].text.as_str(),
                kept[4].text.as_str(),
                kept[6].text.as_str()
            ),
            (DESKTOP_NOT_KEPT, "fn main() {}", DESKTOP_NOT_KEPT)
        );
        assert_eq!(
            history[2].text, "Notepad: meow (unsaved)",
            "the conversation in memory is untouched"
        );
    }
}

#[cfg(test)]
mod computer_use_tests {
    use vivianne_core::{Mode, is_look_only, returns_desktop_content};
    use vivianne_tools::computer::OFFERED;

    /// The tools crate decides what the model is offered, core decides what asks; they cannot see each other, so this
    /// pins them together: only the tools that merely observe run without asking.
    #[test]
    fn every_offered_desktop_tool_is_classified_the_way_it_should_be() {
        let looking = [
            "snapshot",
            "find",
            "get",
            "is",
            "wait",
            "list_apps",
            "list_windows",
        ];
        for tool in OFFERED {
            let name = format!("{}{tool}", vivianne_core::COMPUTER_PREFIX);
            assert_eq!(is_look_only(&name), looking.contains(&tool), "{tool}");
            assert_eq!(
                Mode::Ask.needs_approval(&name),
                !looking.contains(&tool),
                "{tool}"
            );
        }
        // The browser's tools: the same rule, with what a page said never kept.
        for tool in vivianne_tools::browser::OFFERED {
            let name = format!("{}{tool}", vivianne_core::BROWSER_PREFIX);
            assert!(returns_desktop_content(&name), "{tool}");
            let reads_the_open_page = vivianne_core::is_look_only(&name);
            assert_eq!(
                Mode::Ask.needs_approval(&name),
                !reads_the_open_page,
                "{tool}"
            );
        }
        // What a screenshot shows is private even though taking one asks.
        assert!(returns_desktop_content("mcp__computer__desktop_screenshot"));
    }
}
