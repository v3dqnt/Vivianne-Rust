//! The Overview's "Background jobs" section: what the active thread's agent has running.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, div, px};

use super::{muted, section};
use crate::theme::{self, color};

/// One running job as the section shows it. `seconds` is part of it so a redraw follows the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub label: String,
    pub kind: String,
    pub stopping: bool,
    pub seconds: u64,
}

/// `75` as `1m 15s`.
fn elapsed(seconds: u64) -> String {
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

/// The section, or nothing when nothing runs.
pub(super) fn section_of(jobs: &[JobRow]) -> Option<AnyElement> {
    if jobs.is_empty() {
        return None;
    }
    let rows = jobs.iter().map(|job| {
        // A command may be many lines; its first says which it is.
        let first = job.label.lines().next().unwrap_or_default().to_owned();
        let state = if job.stopping { "stopping" } else { "running" };
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::TEXT))
                    .child(first),
            )
            .child(
                muted(format!("{} · {state} · {}", job.kind, elapsed(job.seconds)))
                    .text_size(px(theme::TEXT_XS)),
            )
            .into_any_element()
    });
    Some(section("Background jobs", rows).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::elapsed;

    #[test]
    fn the_time_reads_in_the_two_largest_units() {
        assert_eq!(elapsed(5), "5s");
        assert_eq!(elapsed(75), "1m 15s");
        assert_eq!(elapsed(3 * 3600 + 120), "3h 2m");
    }
}
