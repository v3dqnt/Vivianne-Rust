//! Byte accounting for what a program prints and returns. The cap covers the whole outer result as it would be serialized
//! (logs array, completion value, diagnostic), so a runaway program cannot make a result larger than the caller agreed to.
//! Port of the original `output-json.ts` and `output-ledger.ts`.

use std::io::Write;

use serde_json::Value;

use super::types::{Failure, FailureKind, RunResult};

/// Serialized bytes of one character inside JSON quotes.
fn character_bytes(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// The exact serialized size of `text` as a JSON string (quotes included), or `None` once it passes `max_bytes`.
#[must_use]
pub fn json_string_bytes_up_to(text: &str, max_bytes: usize) -> Option<usize> {
    if max_bytes < 2 {
        return None;
    }
    let mut bytes = 2;
    for c in text.chars() {
        bytes += character_bytes(c);
        if bytes > max_bytes {
            return None;
        }
    }
    Some(bytes)
}

/// The longest whole-character prefix of `text` whose serialized form fits `max_bytes`.
#[must_use]
pub fn truncate_json_string_bytes(text: &str, max_bytes: usize) -> String {
    let mut bytes = 2;
    let mut end = 0;
    for (index, c) in text.char_indices() {
        bytes += character_bytes(c);
        if bytes > max_bytes {
            break;
        }
        end = index + c.len_utf8();
    }
    text[..end].to_owned()
}

/// A writer that only counts, and refuses once the cap is crossed.
struct Counter {
    seen: usize,
    max: usize,
}

impl Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.seen += buf.len();
        if self.seen > self.max {
            return Err(std::io::Error::other("over the cap"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The exact serialized size of a JSON value, or `None` once it passes `max_bytes`; the serialized form is never built.
#[must_use]
pub fn json_value_bytes_up_to(value: &Value, max_bytes: usize) -> Option<usize> {
    let mut counter = Counter {
        seen: 0,
        max: max_bytes,
    };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.seen)
}

/// One run's combined output budget. Values the program passes to bindings never enter it.
#[derive(Debug)]
pub struct OutputLedger {
    bytes: usize,
    entries: usize,
    max_bytes: usize,
}

impl OutputLedger {
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        // `[]`, the serialization of the empty logs array.
        Self {
            bytes: 2,
            entries: 0,
            max_bytes,
        }
    }

    /// Takes one log entry whole, or `false` when it would cross the cap (nothing is taken then).
    pub fn admit(&mut self, text: &str, sink: &mut Vec<String>) -> bool {
        let separator = usize::from(self.entries > 0);
        let Some(size) =
            json_string_bytes_up_to(text, self.max_bytes.saturating_sub(self.bytes + separator))
        else {
            return false;
        };
        self.bytes += size + separator;
        self.entries += 1;
        sink.push(text.to_owned());
        true
    }

    /// Takes as much of `text` as fits, as a prefix. `true` when all of it fit.
    pub fn admit_prefix(&mut self, text: &str, sink: &mut Vec<String>) -> bool {
        if self.admit(text, sink) {
            return true;
        }
        let separator = usize::from(self.entries > 0);
        let prefix =
            truncate_json_string_bytes(text, self.max_bytes.saturating_sub(self.bytes + separator));
        if !prefix.is_empty() {
            self.admit(&prefix, sink);
        }
        false
    }

    /// Finishes a run that returned `value` (or nothing), failing it if the value would not fit.
    #[must_use]
    pub fn success(&self, logs: Vec<String>, value: Option<Value>) -> RunResult {
        if let Some(value) = &value
            && json_value_bytes_up_to(value, self.max_bytes.saturating_sub(self.bytes)).is_none()
        {
            return self.limit(logs);
        }
        RunResult {
            value,
            logs,
            error: None,
        }
    }

    /// Finishes a failed run; a diagnostic that does not fit becomes an output-limit failure instead.
    #[must_use]
    pub fn failure(&self, logs: Vec<String>, error: Failure) -> RunResult {
        if json_string_bytes_up_to(&error.message, self.max_bytes.saturating_sub(self.bytes))
            .is_none()
        {
            return self.limit(logs);
        }
        RunResult {
            value: None,
            logs,
            error: Some(error),
        }
    }

    /// The output-limit failure, keeping the logs that fit with a prefix of the one that did not.
    #[must_use]
    pub fn limit(&self, logs: Vec<String>) -> RunResult {
        let full = format!("outer output exceeded {} bytes", self.max_bytes);
        // The message is ASCII: one byte a character, plus the quotes.
        let log_budget = self.max_bytes.saturating_sub(full.len() + 2);
        let mut retained: Vec<String> = Vec::new();
        let mut used = 2;
        for text in logs {
            let separator = usize::from(!retained.is_empty());
            let available = log_budget.saturating_sub(used + separator);
            if let Some(size) = json_string_bytes_up_to(&text, available) {
                used += size + separator;
                retained.push(text);
                continue;
            }
            let prefix = truncate_json_string_bytes(&text, available);
            if !prefix.is_empty() {
                used += json_string_bytes_up_to(&prefix, available).unwrap_or(0) + separator;
                retained.push(prefix);
            }
            break;
        }
        let message = truncate_json_string_bytes(&full, self.max_bytes.saturating_sub(used));
        RunResult {
            value: None,
            logs: retained,
            error: Some(Failure {
                kind: FailureKind::OutputLimit,
                message,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_string_is_measured_as_it_would_be_serialized() {
        for text in [
            "",
            "abc",
            "quote \" and \\ slash",
            "line\nbreak\ttab",
            "bell\u{7}",
            "é€😀",
            "\u{1}\u{1f}",
        ] {
            let exact = serde_json::to_string(text).expect("json").len();
            assert_eq!(
                json_string_bytes_up_to(text, 10_000),
                Some(exact),
                "{text:?}"
            );
        }
        assert_eq!(json_string_bytes_up_to("abcd", 5), None);
        assert_eq!(json_string_bytes_up_to("abc", 5), Some(5));
        assert_eq!(
            json_string_bytes_up_to("", 1),
            None,
            "the quotes alone need two"
        );
    }

    #[test]
    fn a_prefix_is_cut_on_a_character_and_always_fits() {
        for text in ["plain text", "é€😀 and more", "tab\there \"quoted\""] {
            for budget in 0..40 {
                let cut = truncate_json_string_bytes(text, budget);
                assert!(text.starts_with(&cut));
                if budget >= 2 {
                    assert!(
                        json_string_bytes_up_to(&cut, budget).is_some(),
                        "{text:?} in {budget}"
                    );
                }
            }
        }
        assert_eq!(truncate_json_string_bytes("abcdef", 5), "abc");
    }

    #[test]
    fn a_value_is_measured_without_building_it() {
        let value = json!({ "a": [1, 2.5, null, "x"], "b": { "c": true } });
        assert_eq!(
            json_value_bytes_up_to(&value, 1000),
            Some(serde_json::to_string(&value).expect("json").len())
        );
        assert_eq!(json_value_bytes_up_to(&value, 5), None);
    }

    #[test]
    fn the_ledger_takes_entries_whole_until_the_cap_and_never_half_of_one() {
        let mut ledger = OutputLedger::new(20);
        let mut logs = Vec::new();
        assert!(ledger.admit("abcde", &mut logs)); // 2 + 7
        assert!(ledger.admit("fghij", &mut logs)); // + 1 + 7
        assert!(!ledger.admit("klmnop", &mut logs), "that would make it 25");
        assert_eq!(logs, ["abcde", "fghij"]);
    }

    #[test]
    fn a_prefix_of_the_entry_that_crossed_the_cap_is_kept() {
        let mut ledger = OutputLedger::new(10);
        let mut logs = Vec::new();
        assert!(!ledger.admit_prefix("0123456789", &mut logs));
        assert_eq!(
            logs,
            ["012345"],
            "as much as fits after the brackets and the quotes"
        );
    }

    #[test]
    fn a_value_that_does_not_fit_beside_the_logs_is_an_output_limit_failure() {
        let mut ledger = OutputLedger::new(60);
        let mut logs = Vec::new();
        ledger.admit("hello", &mut logs);
        assert_eq!(
            ledger.success(logs.clone(), Some(json!(1))).value,
            Some(json!(1))
        );
        let big = json!("x".repeat(100));
        let result = ledger.success(logs, Some(big));
        assert_eq!(
            result.error.as_ref().map(|e| e.kind),
            Some(FailureKind::OutputLimit)
        );
        assert!(result.value.is_none() && !result.logs.is_empty());
    }

    #[test]
    fn a_diagnostic_that_does_not_fit_becomes_an_output_limit_failure_and_the_result_stays_inside_the_cap()
     {
        let ledger = OutputLedger::new(80);
        let long = Failure {
            kind: FailureKind::Exception,
            message: "e".repeat(500),
        };
        let result = ledger.failure(vec!["a".repeat(200)], long);
        let error = result.error.expect("failed");
        assert_eq!(error.kind, FailureKind::OutputLimit);
        let total = serde_json::to_string(&result.logs).expect("json").len()
            + serde_json::to_string(&error.message).expect("json").len();
        assert!(total <= 80, "{total}");
        let small = ledger.failure(
            Vec::new(),
            Failure {
                kind: FailureKind::Exception,
                message: "boom".into(),
            },
        );
        assert_eq!(small.error.map(|e| e.kind), Some(FailureKind::Exception));
    }
}
