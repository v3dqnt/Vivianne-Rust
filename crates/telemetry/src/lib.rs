//! Telemetry, crash records and bug reports: one local-first store, ported from the old app.
//!
//! **Nothing leaves this computer unless the user turns it on.** Every event is appended to
//! `~/.viv/agent/telemetry.jsonl` and every crash written to `~/.viv/agent/crashes/` (the old app's files, read
//! in place). Upload happens only for a bug report the user sends, and only when `telemetryEnabled` is true
//! **and** an `https://` `telemetryEndpoint` is set; both default to off, so a fresh install is silent by
//! construction. See [`report`].
//!
//! Three kinds of record answer one question, what happened before things went wrong:
//! - **events**: [`track`], a small fixed set of coarse breadcrumbs. Never message content: the properties are
//!   cut down to numbers, booleans and short identifiers (see `props`).
//! - **crashes**: [`install_panic_hook`], every panic with its backtrace, written before the process dies.
//! - **reports**: [`submit_report`], a user-written description, with recent events and the last crash if asked.
//!
//! What changed from the old code: an append no longer reads the whole file (the file is trimmed by size, rarely);
//! appends from several threads take one lock; properties are restricted; upload uses the app's shared HTTP
//! client and refuses anything but `https://`; crash files are written through a temp file and a rename.

mod props;
mod report;
mod store;

use std::sync::OnceLock;

use serde_json::Value;

pub use report::{Report, upload, upload_target};
pub use store::{CrashRecord, TelemetryEvent};

use store::{Store, now_ms};

/// The version the panic hook was given, so a report names the build that wrote it.
static VERSION: OnceLock<String> = OnceLock::new();

/// Longest panic message and backtrace kept in a crash file.
const MAX_MESSAGE_CHARS: usize = 500;
const MAX_BACKTRACE_CHARS: usize = 20_000;

fn version() -> &'static str {
    VERSION.get().map_or(env!("CARGO_PKG_VERSION"), String::as_str)
}

/// Records one event. Best-effort by design: telemetry must never fail a user action, so every error is
/// swallowed after a debug log. `props` is cleaned first: only numbers, booleans and short identifiers stay.
pub fn track(kind: &str, props: Value) {
    let result = Store::standard().and_then(|store| store.track(kind, &props));
    if let Err(e) = result {
        log::debug!("[telemetry] dropped '{kind}': {e}");
    }
}

/// Every recorded event, oldest first.
#[must_use]
pub fn read_events() -> Vec<TelemetryEvent> {
    Store::standard().map(|store| store.read_events()).unwrap_or_default()
}

/// Every recorded crash, newest first.
#[must_use]
pub fn read_crashes() -> Vec<CrashRecord> {
    Store::standard().map(|store| store.read_crashes()).unwrap_or_default()
}

/// Deletes every event and crash on this computer.
pub fn clear() -> Result<(), String> {
    Store::standard()?.clear()
}

/// What is recorded, for the Diagnostics page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub event_count: usize,
    pub first_event_ts: Option<u64>,
    pub last_event_ts: Option<u64>,
    /// Counts by event kind, most frequent first (ties by name).
    pub kinds: Vec<(String, usize)>,
    /// Newest first.
    pub crashes: Vec<CrashRecord>,
}

/// Counts by kind, first and last event time, and the crashes.
#[must_use]
pub fn summary() -> Summary {
    summarise(&read_events(), read_crashes())
}

fn summarise(events: &[TelemetryEvent], crashes: Vec<CrashRecord>) -> Summary {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for event in events {
        *counts.entry(event.kind.as_str()).or_default() += 1;
    }
    let mut kinds: Vec<(String, usize)> = counts.into_iter().map(|(kind, count)| (kind.to_owned(), count)).collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Summary {
        event_count: events.len(),
        first_event_ts: events.first().map(|e| e.ts),
        last_event_ts: events.last().map(|e| e.ts),
        kinds,
        crashes,
    }
}

/// Records every panic to disk. The record is written before the panic goes on, so a crash that takes the
/// process down still leaves evidence. Call once, first thing in `main`.
pub fn install_panic_hook(app_version: &str) {
    let _ = VERSION.set(app_version.to_owned());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_owned());
        let record = CrashRecord {
            ts: now_ms(),
            message: message.chars().take(MAX_MESSAGE_CHARS).collect(),
            location: info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default(),
            backtrace: std::backtrace::Backtrace::force_capture().to_string().chars().take(MAX_BACKTRACE_CHARS).collect(),
            app_version: version().to_owned(),
            os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        };
        if let Err(e) = Store::standard().and_then(|store| store.write_crash(&record)) {
            log::error!("[telemetry] could not record crash: {e}");
        }
        // Chain to the earlier hook so the panic still prints as usual: recording it must not hide it
        // from anyone watching a console.
        previous(info);
    }));
}

/// Bundles a user-written report with the context needed to act on it (recent events and the latest crash when
/// `include_diagnostics`), saves it under `~/.viv/agent/reports/`, then sends it if the user opted in. The file
/// is written first, so the report survives an upload that is off or fails.
pub async fn submit_report(description: &str, include_diagnostics: bool) -> Result<Report, String> {
    let store = Store::standard()?;
    report::submit(&store, description, include_diagnostics, version(), upload).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(ts: u64, kind: &str) -> TelemetryEvent {
        TelemetryEvent { ts, kind: kind.to_owned(), props: Value::Null }
    }

    #[test]
    fn the_summary_counts_by_kind_most_frequent_first_with_the_first_and_last_time() {
        let events = [event(10, "app.start"), event(20, "turn.complete"), event(30, "turn.complete"), event(40, "thread.new")];
        let summary = summarise(&events, Vec::new());
        assert_eq!(summary.event_count, 4);
        assert_eq!((summary.first_event_ts, summary.last_event_ts), (Some(10), Some(40)));
        assert_eq!(summary.kinds, [("turn.complete".to_owned(), 2), ("app.start".to_owned(), 1), ("thread.new".to_owned(), 1)]);
    }

    #[test]
    fn an_empty_store_summarises_to_nothing() {
        assert_eq!(summarise(&[], Vec::new()), Summary::default());
    }
}
