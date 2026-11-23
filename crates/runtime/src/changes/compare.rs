//! Line comparison of two whole-file texts, bounded by a timeout that degrades to whole-file replacement.

use std::time::{Duration, Instant};

use similar::{ChangeTag, TextDiff};

use super::types::Hunk;

/// Context lines around each change, the unified-diff default.
const CONTEXT_LINES: usize = 3;

/// Hunks, whether the timeout degraded them, and the changed-line totals they carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub hunks: Vec<Hunk>,
    pub coarse: bool,
    pub added: usize,
    pub deleted: usize,
}

/// Every line terminated, so the last line compares by content alone and empty text reads as no lines.
fn terminated(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

/// The content lines of a terminated text; empty text is zero lines.
fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text[..text.len() - 1].split('\n').collect()
    }
}

/// Compares two texts line by line. A side that is `None` did not exist. A comparison over `timeout_ms` yields one hunk
/// that deletes every old line and adds every new one. No hunks when both sides hold the same lines.
#[must_use]
pub fn compare_text(before: Option<&str>, after: Option<&str>, timeout_ms: u64) -> Comparison {
    let old = terminated(before.unwrap_or(""));
    let new = terminated(after.unwrap_or(""));
    let timeout = Duration::from_millis(timeout_ms);
    let started = Instant::now();
    let diff = TextDiff::configure()
        .timeout(timeout)
        .diff_lines(&old, &new);
    let hunks: Vec<Hunk> = diff
        .grouped_ops(CONTEXT_LINES)
        .iter()
        .map(|group| {
            let (first, last) = (&group[0], &group[group.len() - 1]);
            let (old_start, new_start) = (first.old_range().start, first.new_range().start);
            let body = group
                .iter()
                .flat_map(|op| diff.iter_changes(op))
                .map(|change| {
                    let sign = match change.tag() {
                        ChangeTag::Delete => '-',
                        ChangeTag::Insert => '+',
                        ChangeTag::Equal => ' ',
                    };
                    format!(
                        "{sign}{}",
                        change.value().strip_suffix('\n').unwrap_or(change.value())
                    )
                })
                .collect();
            Hunk {
                old_start: old_start + 1,
                old_lines: last.old_range().end - old_start,
                new_start: new_start + 1,
                new_lines: last.new_range().end - new_start,
                lines: body,
            }
        })
        .collect();
    // Past its deadline the comparison is only approximate: say so and show the plain replacement.
    if started.elapsed() >= timeout && old != new {
        let (old_lines, new_lines) = (lines(&old), lines(&new));
        let body = old_lines
            .iter()
            .map(|l| format!("-{l}"))
            .chain(new_lines.iter().map(|l| format!("+{l}")))
            .collect();
        return Comparison {
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: old_lines.len(),
                new_start: 1,
                new_lines: new_lines.len(),
                lines: body,
            }],
            coarse: true,
            added: new_lines.len(),
            deleted: old_lines.len(),
        };
    }
    let count = |sign: char| {
        hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.starts_with(sign))
            .count()
    };
    Comparison {
        added: count('+'),
        deleted: count('-'),
        hunks,
        coarse: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_line_is_one_hunk_with_context() {
        let got = compare_text(
            Some("a\nb\nc\nd\ne\nf\ng\n"),
            Some("a\nb\nc\nD\ne\nf\ng\n"),
            1000,
        );
        assert_eq!((got.added, got.deleted, got.coarse), (1, 1, false));
        assert_eq!(got.hunks.len(), 1);
        let h = &got.hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 7, 1, 7)
        );
        assert_eq!(
            h.lines,
            ["", "a", "b", "c"]
                .iter()
                .skip(1)
                .map(|l| format!(" {l}"))
                .chain(["-d".into(), "+D".into()])
                .chain(["e", "f", "g"].iter().map(|l| format!(" {l}")))
                .collect::<Vec<String>>()
        );
    }

    #[test]
    fn a_missing_final_newline_alone_is_not_a_change() {
        assert!(
            compare_text(Some("a\nb"), Some("a\nb\n"), 1000)
                .hunks
                .is_empty()
        );
    }

    #[test]
    fn a_new_file_adds_every_line_and_a_deleted_one_removes_them() {
        let made = compare_text(None, Some("x\ny\n"), 1000);
        assert_eq!((made.added, made.deleted), (2, 0));
        assert_eq!((made.hunks[0].old_lines, made.hunks[0].new_lines), (0, 2));
        let gone = compare_text(Some("x\ny\nz\n"), None, 1000);
        assert_eq!((gone.added, gone.deleted), (0, 3));
        assert!(compare_text(None, None, 1000).hunks.is_empty());
    }

    #[test]
    fn far_apart_changes_are_separate_hunks() {
        let before: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let after = before
            .replace("line 2\n", "two\n")
            .replace("line 35\n", "thirty-five\n");
        assert_eq!(
            compare_text(Some(&before), Some(&after), 1000).hunks.len(),
            2
        );
    }

    #[test]
    fn a_comparison_that_runs_out_of_time_replaces_the_whole_file() {
        let before: String = (0..20_000)
            .map(|i| format!("a {}\n", i * 7 % 1013))
            .collect();
        let after: String = (0..20_000)
            .map(|i| format!("b {}\n", i * 11 % 997))
            .collect();
        let got = compare_text(Some(&before), Some(&after), 1);
        if got.coarse {
            assert_eq!(got.hunks.len(), 1);
            assert_eq!((got.deleted, got.added), (20_000, 20_000));
        }
        // Either way the totals describe what the hunks show.
        let shown_added = got
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.starts_with('+'))
            .count();
        assert_eq!(got.added, shown_added);
    }
}
