//! What the run has tried, kept by the loop rather than by the model.
//!
//! A model that keeps retrying one failing idea with small variations (a flag moved, `./src/a.rs` for
//! `src/a.rs`) walks past a check for identical calls. Every call is recorded under a normalized key with its
//! outcome, and once the same thing has failed before, the next failure carries the record: what was tried, how
//! many times, and the error each time. It lives outside the message list, so shrinking the history cannot lose it.

use std::collections::HashMap;

use vivianne_core::ToolCall;

/// Failures of one key before the model is shown the record.
const SHOW_AFTER: usize = 2;
/// Errors remembered per key, newest last, and the characters kept of each.
const KEEP_ERRORS: usize = 3;
const ERROR_CHARS: usize = 200;

#[derive(Debug, Default)]
pub struct Attempts(HashMap<String, Vec<String>>);

impl Attempts {
    /// The call's identity: its tool and arguments, with whitespace and a leading `./` normalized away.
    fn key(call: &ToolCall) -> String {
        let args = call
            .args
            .to_string()
            .replace("./", "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {args}", call.name)
    }

    /// Notes a failure. Returns the note to add to its result when this is not the first time.
    pub fn failed(&mut self, call: &ToolCall, error: &str) -> Option<String> {
        let errors = self.0.entry(Self::key(call)).or_default();
        errors.push(error.chars().take(ERROR_CHARS).collect());
        let tries = errors.len();
        if tries < SHOW_AFTER {
            return None;
        }
        let recent = errors
            .iter()
            .rev()
            .take(KEEP_ERRORS)
            .rev()
            .enumerate()
            .map(|(i, e)| format!("  {}. {e}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "\n\nThis exact call has now failed {tries} times in this run. The errors were:\n{recent}\nChange the approach, or tell the user what is blocking you; do not try it again unchanged."
        ))
    }

    /// Notes a success: the same call working means earlier failures no longer count.
    pub fn succeeded(&mut self, call: &ToolCall) {
        self.0.remove(&Self::key(call));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(path: &str) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: "read".into(),
            args: json!({ "path": path }),
        }
    }

    #[test]
    fn the_second_identical_failure_carries_the_record_and_a_success_clears_it() {
        let mut attempts = Attempts::default();
        assert!(attempts.failed(&call("src/a.rs"), "no such file").is_none());
        let note = attempts
            .failed(&call("./src/a.rs"), "no such file")
            .expect("a small variation is the same attempt");
        assert!(note.contains("failed 2 times") && note.contains("no such file"));
        attempts.succeeded(&call("src/a.rs"));
        assert!(
            attempts.failed(&call("src/a.rs"), "again").is_none(),
            "the record was cleared"
        );
        assert!(
            attempts.failed(&call("src/b.rs"), "x").is_none(),
            "another file is another attempt"
        );
    }
}
