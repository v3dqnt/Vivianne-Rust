//! Notices a model making the same call again and again, working or not (`attempts` only counts failures), and
//! says so in the result.
//!
//! Port of `@deepseek-ai/dsh-repeat-tool-reminder` with its default thresholds. The original's include and exclude
//! patterns, and configurable thresholds, are not carried over: every tool is tracked.

use serde_json::Value;
use vivianne_core::ToolCall;

/// Consecutive identical calls that draw a reminder: the first gently, the later ones in detail.
const THRESHOLDS: [usize; 3] = [3, 5, 8];
/// Characters of the arguments a detailed reminder shows.
const PREVIEW_CHARS: usize = 500;

const GENTLE: &str = "You are repeating the exact same tool call with identical arguments. Carefully analyze the \
previous result before calling again: if the task is not complete, try a different approach or different arguments \
instead of repeating the call.";

/// The arguments with every object's keys in order, so the order a model wrote them in does not matter.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{}:{}", Value::from(key.as_str()), canonical(&map[key])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

fn preview(arguments: &str) -> String {
    let total = arguments.chars().count();
    if total <= PREVIEW_CHARS {
        return arguments.to_owned();
    }
    let kept: String = arguments.chars().take(PREVIEW_CHARS).collect();
    format!("{kept}… (+{} more chars)", total - PREVIEW_CHARS)
}

/// The run of identical calls going on now.
#[derive(Debug, Default)]
pub struct Repeats {
    key: String,
    count: usize,
}

impl Repeats {
    /// Counts a call that got an answer, whatever it was. Returns what to add to that answer, if anything.
    pub fn observe(&mut self, call: &ToolCall) -> Option<String> {
        let arguments = canonical(&call.args);
        let key = format!("{}\n{arguments}", call.name);
        if key == self.key {
            self.count += 1;
        } else {
            self.key = key;
            self.count = 1;
        }
        let count = self.count;
        if !THRESHOLDS.contains(&count) {
            return None;
        }
        Some(if count == THRESHOLDS[0] {
            format!("\n\n{GENTLE}")
        } else {
            format!(
                "\n\nRepeated tool call detected:\n- tool: {}\n- consecutive_calls: {count}\n- arguments: {}\n\
                 The repeated calls are not making progress. Do not call this tool with these exact arguments again. \
                 Inspect the latest result and choose a different action, different arguments, or finish the task if \
                 enough evidence has been gathered.",
                call.name,
                preview(&arguments)
            )
        })
    }

    /// The person said something: what was a loop may now be asked for.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            args,
        }
    }

    #[test]
    fn the_third_identical_call_is_reminded_gently_and_the_fifth_and_eighth_in_detail() {
        let mut repeats = Repeats::default();
        let notes: Vec<Option<String>> = (0..9)
            .map(|_| repeats.observe(&call("read", json!({"path": "a"}))))
            .collect();
        let said: Vec<usize> = (0..9)
            .filter(|i| notes[*i].is_some())
            .map(|i| i + 1)
            .collect();
        assert_eq!(said, [3, 5, 8]);
        assert!(notes[2].as_deref().is_some_and(|n| n.contains(GENTLE)));
        assert!(
            notes[4].as_deref().is_some_and(
                |n| n.contains("consecutive_calls: 5") && n.contains(r#"{"path":"a"}"#)
            )
        );
    }

    #[test]
    fn another_call_or_the_person_starts_the_count_again_and_key_order_does_not_matter() {
        let mut repeats = Repeats::default();
        let a = call("read", json!({"path": "a", "limit": 1}));
        let same: Value = serde_json::from_str(r#"{"limit":1,"path":"a"}"#).expect("json");
        assert!(repeats.observe(&a).is_none());
        assert!(repeats.observe(&call("read", same)).is_none());
        assert!(repeats.observe(&a).is_some(), "the third of the same");
        assert!(repeats.observe(&call("ls", json!({}))).is_none());
        assert!(repeats.observe(&a).is_none());
        assert!(repeats.observe(&a).is_none());
        repeats.reset();
        assert!(repeats.observe(&a).is_none());
    }

    #[test]
    fn long_arguments_are_cut_in_the_reminder() {
        let cut = preview(&"é".repeat(PREVIEW_CHARS + 7));
        assert!(cut.ends_with("… (+7 more chars)"));
    }
}
