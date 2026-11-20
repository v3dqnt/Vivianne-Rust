//! Daily-aggregated real token/cost usage, kept under `~/.viv/agent`.
//!
//! Three files, all plain JSON, none a database (the only query shapes a usage page needs are "every
//! day", "this model over a range" and "this thread"):
//! - `usage.json`: day -> totals. Kept for ever (a handful of entries a day).
//! - `usage_by_source.json`: day -> `"main:<thread>"` / `"task:<thread>"` -> totals. It gains a key for every
//!   thread ever, so it keeps a rolling [`BY_SOURCE_RETENTION_DAYS`] days and no more.
//! - `usage_by_model.json`: day -> `"<provider>/<model id>"` -> totals. It has its own retention: **everything
//!   is kept**. A day holds one entry per model used, so it grows by a few lines a day, and the "All time"
//!   view of Settings, Account must really be all time (the old app pruned it with the by-source file, so
//!   "All" silently meant 90 days).
//!
//! Who records: the chat session, once per model call, from the usage the turn reports (the provider layer
//! no longer records, so a call is never counted twice). Recording is buffered in memory and written by
//! [`flush_all`]: on a timer ([`flush_periodically`]), at least every [`FLUSH_INTERVAL_MS`] while records
//! keep arriving, and by the app on quit, so the last minutes of a session are not lost.
//!
//! Every write goes through a temp file and a rename ([`crate::paths::write_json_atomic`]), so a crash in the
//! middle of a flush leaves the old file, never half of one.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub requests: u32,
    /// True if at least one request this day contributed to `cost` from a
    /// model `ai::pricing` has no known price for — `cost` is a real but
    /// **partial** total in that case, not the full day's spend. Old records
    /// written before real cost tracking existed (when every `cost` was a
    /// hardcoded 0.0) deserialize this as `true` too, via the default below,
    /// so they don't misrepresent a fake zero as a confirmed real one.
    #[serde(default = "default_true")]
    pub has_unknown_cost: bool,
}

fn default_true() -> bool {
    true
}

impl DayUsage {
    /// `cost` is the real, priced cost for this one request (from
    /// `ai::pricing::compute_cost`, or what the provider reported), or `None` if the model has no known
    /// price — accumulated honestly rather than silently treated as $0.
    fn accumulate(&mut self, usage: &crate::ai::types::Usage, cost: Option<f64>) {
        self.input += u64::from(usage.input);
        self.output += u64::from(usage.output);
        self.cache_read += u64::from(usage.cache_read);
        self.cache_write += u64::from(usage.cache_write);
        match cost {
            Some(c) => self.cost += c,
            None => self.has_unknown_cost = true,
        }
        self.requests += 1;
    }

    /// Every token this record counts, input and output (what a "N tokens" line means).
    #[must_use]
    pub fn tokens(&self) -> u64 {
        self.input + self.output
    }
}

type UsageBySource = BTreeMap<String, BTreeMap<String, DayUsage>>;

/// Where the three files live.
struct Files {
    days: PathBuf,
    by_source: PathBuf,
    by_model: PathBuf,
}

impl Files {
    fn in_dir(dir: &Path) -> Self {
        Self {
            days: dir.join("usage.json"),
            by_source: dir.join("usage_by_source.json"),
            by_model: dir.join("usage_by_model.json"),
        }
    }

    fn standard() -> Result<Self, String> {
        Ok(Self::in_dir(&crate::paths::viv_agent_dir()?))
    }
}

lazy_static::lazy_static! {
    /// In-memory write buffer + last-flush timestamp.
    /// Uses `tokio::sync::Mutex` so flush I/O can happen while holding the lock.
    static ref USAGE_BUFFER: AsyncMutex<BufferState> = AsyncMutex::new(BufferState::new());
}

/// Minimum interval between flushes triggered by a record (milliseconds).
const FLUSH_INTERVAL_MS: u64 = 5_000;

/// How often [`flush_periodically`] writes what is buffered, even when no record arrives.
const PERIODIC_FLUSH: Duration = Duration::from_secs(30);

struct BufferState {
    days: BTreeMap<String, DayUsage>,
    by_source: UsageBySource,
    by_model: UsageBySource,
    last_flush: Instant,
}

impl BufferState {
    fn new() -> Self {
        Self {
            days: BTreeMap::new(),
            by_source: BTreeMap::new(),
            by_model: BTreeMap::new(),
            last_flush: Instant::now(),
        }
    }

    fn is_empty(&self) -> bool {
        self.days.is_empty() && self.by_source.is_empty() && self.by_model.is_empty()
    }
}

async fn read_all_from(path: &Path) -> BTreeMap<String, DayUsage> {
    let Ok(content) = tokio::fs::read_to_string(path).await else { return BTreeMap::new() };
    serde_json::from_str::<BTreeMap<String, DayUsage>>(&content).unwrap_or_default()
}

async fn read_all_by_source_from(path: &Path) -> UsageBySource {
    let Ok(content) = tokio::fs::read_to_string(path).await else { return BTreeMap::new() };
    serde_json::from_str::<UsageBySource>(&content).unwrap_or_default()
}

/// Fold `src` into `target` — the one place all 6 fields get summed, reused
/// by every merge site below instead of repeating the field list at each one
/// (and silently dropping `has_unknown_cost` the way a naive copy-paste of
/// the other 5 fields would).
fn merge_into(target: &mut DayUsage, src: &DayUsage) {
    target.input += src.input;
    target.output += src.output;
    target.cache_read += src.cache_read;
    target.cache_write += src.cache_write;
    target.cost += src.cost;
    target.requests += src.requests;
    target.has_unknown_cost = target.has_unknown_cost || src.has_unknown_cost;
}

/// Fold a nested buffer (day -> key -> totals) into `target`.
fn merge_nested(target: &mut UsageBySource, buffer: &UsageBySource) {
    for (day, entries) in buffer {
        let day_entry = target.entry(day.clone()).or_default();
        for (key, usage) in entries {
            merge_into(day_entry.entry(key.clone()).or_default(), usage);
        }
    }
}

/// Writes `value` to `path` through a temp file and a rename, off the async threads.
async fn write_atomic<T: Serialize + Send + 'static>(path: &Path, value: T) -> Result<(), String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || crate::paths::write_json_atomic(&path, &value)).await.map_err(|e| e.to_string())?
}

async fn merge_and_write(path: &Path, buffer: &BTreeMap<String, DayUsage>) -> Result<(), String> {
    let mut disk = read_all_from(path).await;
    for (day, usage) in buffer {
        merge_into(disk.entry(day.clone()).or_default(), usage);
    }
    write_atomic(path, disk).await
}

/// `by_source` gains a brand-new key (`"main:<thread>"` / `"task:<thread>"`) for every thread/subagent run
/// ever, with no natural expiry — unlike `usage.json`'s day totals, nothing here ever collapses old entries
/// together. Bounding it to a rolling window is what keeps its read-merge-write on every flush from growing
/// indefinitely across the file's whole lifetime; a per-thread view has no use for a hundred-day-old tag.
/// (Only this file: `usage_by_model.json` keeps everything, see the module doc.)
const BY_SOURCE_RETENTION_DAYS: i64 = 90;

/// Forgets every day before `cutoff` (`YYYY-MM-DD`).
fn prune_before(disk: &mut UsageBySource, cutoff: &str) {
    disk.retain(|day, _| day.as_str() >= cutoff);
}

fn by_source_cutoff() -> String {
    days_ago_key(BY_SOURCE_RETENTION_DAYS)
}

/// Merges `buffer` into the nested file at `path`; `cutoff` is the oldest day to keep, `None` keeps all.
async fn merge_and_write_nested(path: &Path, buffer: &UsageBySource, cutoff: Option<&str>) -> Result<(), String> {
    let mut disk = read_all_by_source_from(path).await;
    merge_nested(&mut disk, buffer);
    if let Some(cutoff) = cutoff {
        prune_before(&mut disk, cutoff);
    }
    write_atomic(path, disk).await
}

/// Writes whatever is buffered to `files`. A buffer is cleared only once its file is written, so a failed
/// write is retried by the next flush rather than lost.
async fn flush_to(state: &mut BufferState, files: &Files, what: &str) {
    if !state.days.is_empty() {
        match merge_and_write(&files.days, &state.days).await {
            Ok(()) => state.days.clear(),
            Err(e) => log::warn!("[usage] {what} failed: {e}"),
        }
    }
    if !state.by_source.is_empty() {
        match merge_and_write_nested(&files.by_source, &state.by_source, Some(&by_source_cutoff())).await {
            Ok(()) => state.by_source.clear(),
            Err(e) => log::warn!("[usage] {what} by-source failed: {e}"),
        }
    }
    if !state.by_model.is_empty() {
        match merge_and_write_nested(&files.by_model, &state.by_model, None).await {
            Ok(()) => state.by_model.clear(),
            Err(e) => log::warn!("[usage] {what} by-model failed: {e}"),
        }
    }
    state.last_flush = Instant::now();
}

async fn maybe_flush(state: &mut BufferState) {
    if state.last_flush.elapsed() < Duration::from_millis(FLUSH_INTERVAL_MS) {
        return;
    }
    if let Ok(files) = Files::standard() {
        flush_to(state, &files, "flush").await;
    }
}

/// What a request cost: the figure the provider reported, else the price table's, else unknown.
fn cost_of(usage: &crate::ai::types::Usage, provider: &str, model_id: &str) -> Option<f64> {
    usage.cost.as_ref().map(|c| c.total).or_else(|| crate::ai::pricing::compute_cost(provider, model_id, usage))
}

fn is_empty(usage: &crate::ai::types::Usage) -> bool {
    usage.input == 0 && usage.output == 0 && usage.total_tokens == 0
}

/// Today's date, `YYYY-MM-DD` in local time: the key a day's usage is filed under.
#[must_use]
pub fn today_key() -> String {
    days_ago_key(0)
}

/// The date `days` days before today, in the same form.
#[must_use]
pub fn days_ago_key(days: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
}

/// Accumulate `usage` into today's bucket (local date) and today's per-model bucket, pricing it via the
/// cost the provider reported or `ai::pricing::compute_cost(provider, model_id, usage)` — real cost when the
/// model is known, honestly tracked as unknown otherwise (see `DayUsage::has_unknown_cost`) rather than
/// silently recorded as $0. Best-effort — a failure here should never break a turn, so this only ever logs.
pub async fn record_usage(usage: &crate::ai::types::Usage, provider: &str, model_id: &str) {
    if is_empty(usage) {
        return;
    }
    let cost = cost_of(usage, provider, model_id);
    let today = today_key();
    let mut state = USAGE_BUFFER.lock().await;
    state.by_model.entry(today.clone()).or_default().entry(format!("{provider}/{model_id}")).or_default().accumulate(usage, cost);
    state.days.entry(today).or_default().accumulate(usage, cost);
    maybe_flush(&mut state).await;
}

/// Full history for the frontend's usage dashboard.
pub async fn read_history() -> BTreeMap<String, DayUsage> {
    let Ok(files) = Files::standard() else { return BTreeMap::new() };
    let mut disk = read_all_from(&files.days).await;
    let state = USAGE_BUFFER.lock().await;
    for (day, usage) in &state.days {
        merge_into(disk.entry(day.clone()).or_default(), usage);
    }
    disk
}

/// Thread/subagent-attributed usage, parallel to `record_usage` — purely
/// additive telemetry. `source_tag` is a free-form label
/// (`"main:<thread>"`, `"task:<thread>"`).
/// Best-effort — a failure here should never break a turn.
pub async fn record_usage_tagged(usage: &crate::ai::types::Usage, provider: &str, model_id: &str, source_tag: &str) {
    if is_empty(usage) {
        return;
    }
    let cost = cost_of(usage, provider, model_id);
    let mut state = USAGE_BUFFER.lock().await;
    state.by_source.entry(today_key()).or_default().entry(source_tag.to_string()).or_default().accumulate(usage, cost);
    maybe_flush(&mut state).await;
}

/// Full source-attributed history (the last [`BY_SOURCE_RETENTION_DAYS`] days).
pub async fn read_history_by_source() -> UsageBySource {
    let Ok(files) = Files::standard() else { return BTreeMap::new() };
    read_by_source_in(&files).await
}

async fn read_by_source_in(files: &Files) -> UsageBySource {
    let mut disk = read_all_by_source_from(&files.by_source).await;
    let state = USAGE_BUFFER.lock().await;
    merge_nested(&mut disk, &state.by_source);
    disk
}

/// What one thread has used: its `main:<thread>` and `task:<thread>` records summed across the days kept.
/// The thread's id must be unique across launches, or an older thread's usage would be counted in.
pub async fn thread_total(thread: &str) -> DayUsage {
    sum_thread(&read_history_by_source().await, thread)
}

/// Sums `main:<thread>` and `task:<thread>` over every day. Whole tags only, so thread 1 never takes in
/// thread 10.
fn sum_thread(history: &UsageBySource, thread: &str) -> DayUsage {
    let tags = [format!("main:{thread}"), format!("task:{thread}")];
    let mut total = DayUsage { has_unknown_cost: false, ..DayUsage::default() };
    for sources in history.values() {
        for tag in &tags {
            if let Some(usage) = sources.get(tag) {
                merge_into(&mut total, usage);
            }
        }
    }
    total
}

/// Flush the in-memory buffer to disk immediately. Called when the app quits to ensure no usage data is lost.
pub async fn flush_all() {
    let mut state = USAGE_BUFFER.lock().await;
    if state.is_empty() {
        return;
    }
    if let Ok(files) = Files::standard() {
        flush_to(&mut state, &files, "final flush").await;
    }
}

/// Writes the buffer every [`PERIODIC_FLUSH`] for as long as it runs: a flush that waited for the next record
/// would never happen after the last one of a session. Spawn it once on the async runtime.
pub async fn flush_periodically() {
    loop {
        tokio::time::sleep(PERIODIC_FLUSH).await;
        flush_all().await;
    }
}

/// Usage per model since `since_day` (`YYYY-MM-DD`, inclusive; `None` is
/// everything kept, which is everything ever recorded), summed across days.
pub async fn read_usage_by_model(since_day: Option<&str>) -> BTreeMap<String, DayUsage> {
    let mut disk = match Files::standard() {
        Ok(files) => read_all_by_source_from(&files.by_model).await,
        Err(_) => BTreeMap::new(),
    };
    let state = USAGE_BUFFER.lock().await;
    merge_nested(&mut disk, &state.by_model);
    fold_by_model(&disk, since_day)
}

/// The per-day per-model history folded to one total per model, from
/// `since_day` on.
fn fold_by_model(history: &UsageBySource, since_day: Option<&str>) -> BTreeMap<String, DayUsage> {
    let mut out: BTreeMap<String, DayUsage> = BTreeMap::new();
    for (day, models) in history {
        if since_day.is_some_and(|s| day.as_str() < s) {
            continue;
        }
        for (model, usage) in models {
            merge_into(out.entry(model.clone()).or_default(), usage);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32) -> crate::ai::types::Usage {
        crate::ai::types::Usage { input, output, cache_read: 0, cache_write: 0, total_tokens: input + output, cost: None }
    }

    fn temp_files() -> (PathBuf, Files) {
        let dir = std::env::temp_dir().join(format!("vivianne_usage_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let files = Files::in_dir(&dir);
        (dir, files)
    }

    fn one(day: &str, key: &str, input: u32) -> UsageBySource {
        let mut map: UsageBySource = BTreeMap::new();
        map.entry(day.to_string()).or_default().entry(key.to_string()).or_default().accumulate(&usage(input, 1), Some(0.01));
        map
    }

    #[test]
    fn accumulate_sums_across_multiple_requests() {
        let mut day = DayUsage::default();
        day.accumulate(&usage(100, 50), Some(0.01));
        day.accumulate(&usage(200, 75), Some(0.02));
        assert_eq!(day.input, 300);
        assert_eq!(day.output, 125);
        assert_eq!(day.requests, 2);
        assert!((day.cost - 0.03).abs() < 1e-9);
        assert!(!day.has_unknown_cost, "both requests had a known cost");
    }

    #[test]
    fn an_unknown_priced_request_taints_the_whole_day() {
        let mut day = DayUsage::default();
        day.accumulate(&usage(100, 50), Some(0.01));
        day.accumulate(&usage(200, 75), None); // model with no known price
        assert!(day.has_unknown_cost, "one unknown-priced request makes the day's total partial");
        assert!((day.cost - 0.01).abs() < 1e-9, "the known-cost request's contribution is still counted");
    }

    #[test]
    fn a_reported_cost_wins_over_the_price_table() {
        let mut with_cost = usage(10, 10);
        with_cost.cost = Some(crate::ai::types::CostBreakdown { total: 0.25, ..Default::default() });
        assert_eq!(cost_of(&with_cost, "harness", "claude-code"), Some(0.25));
        assert_eq!(cost_of(&usage(10, 10), "harness", "claude-code"), None, "no price and no report is unknown, not zero");
    }

    #[test]
    fn a_thread_total_sums_its_own_main_and_task_tags_across_days() {
        let mut history = one("2026-10-10", "main:thread-1", 100);
        merge_nested(&mut history, &one("2026-10-11", "main:thread-1", 200));
        merge_nested(&mut history, &one("2026-10-11", "task:thread-1", 50));
        merge_nested(&mut history, &one("2026-10-11", "main:thread-10", 9_999));
        merge_nested(&mut history, &one("2026-10-11", "main:thread-2", 7_777));

        let total = sum_thread(&history, "thread-1");
        assert_eq!(total.input, 350, "thread-10 and thread-2 are other threads");
        assert_eq!(total.requests, 3);
        assert!((total.cost - 0.03).abs() < 1e-9);
        assert!(!total.has_unknown_cost);

        let none = sum_thread(&history, "thread-3");
        assert_eq!(none.requests, 0);
    }

    #[test]
    fn a_thread_with_an_unpriced_call_says_its_cost_is_partial() {
        let mut history: UsageBySource = BTreeMap::new();
        history.entry("2026-10-11".into()).or_default().entry("main:t".into()).or_default().accumulate(&usage(5, 5), None);
        assert!(sum_thread(&history, "t").has_unknown_cost);
    }

    #[test]
    fn per_model_totals_keep_models_apart_and_respect_the_range() {
        let mut history: UsageBySource = BTreeMap::new();
        let mut put = |day: &str, model: &str, input: u32, cost: Option<f64>| {
            history.entry(day.to_string()).or_default().entry(model.to_string()).or_default().accumulate(&usage(input, 10), cost);
        };
        put("2026-10-10", "nebius/nvidia/nemotron-3-ultra", 1000, Some(0.50));
        put("2026-10-11", "nebius/nvidia/nemotron-3-ultra", 500, Some(0.25));
        put("2026-10-11", "nebius/nvidia/nemotron-3-nano", 4000, None);

        let all = fold_by_model(&history, None);
        assert_eq!(all.len(), 2);
        assert_eq!(all["nebius/nvidia/nemotron-3-ultra"].input, 1500);
        assert_eq!(all["nebius/nvidia/nemotron-3-ultra"].requests, 2);
        assert!((all["nebius/nvidia/nemotron-3-ultra"].cost - 0.75).abs() < 1e-9);
        assert!(all["nebius/nvidia/nemotron-3-nano"].has_unknown_cost, "an unpriced model is flagged, not shown as free");

        // A range keeps only the later day.
        let recent = fold_by_model(&history, Some("2026-10-11"));
        assert_eq!(recent["nebius/nvidia/nemotron-3-ultra"].input, 500);
    }

    #[test]
    fn zero_usage_is_not_recorded() {
        assert!(is_empty(&usage(0, 0)));
        assert!(!is_empty(&usage(1, 0)));
    }

    #[tokio::test]
    async fn a_flush_writes_every_file_merges_with_what_is_there_and_empties_the_buffer() {
        let (dir, files) = temp_files();
        let mut state = BufferState::new();
        state.days.entry("2026-11-01".into()).or_default().accumulate(&usage(10, 5), Some(0.001));
        state.by_source = one("2026-11-01", "main:t", 10);
        state.by_model = one("2026-11-01", "p/m", 10);
        flush_to(&mut state, &files, "test").await;
        assert!(state.is_empty(), "written buffers are cleared");

        state.days.entry("2026-11-01".into()).or_default().accumulate(&usage(20, 10), Some(0.002));
        state.by_source = one("2026-11-01", "main:t", 20);
        flush_to(&mut state, &files, "test").await;

        let days = read_all_from(&files.days).await;
        assert_eq!((days["2026-11-01"].input, days["2026-11-01"].requests), (30, 2));
        let by_source = read_all_by_source_from(&files.by_source).await;
        assert_eq!(by_source["2026-11-01"]["main:t"].input, 30);
        assert!(read_all_by_source_from(&files.by_model).await.contains_key("2026-11-01"));
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "tmp")).collect();
        assert!(leftovers.is_empty(), "the temp file was renamed over the real one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn by_source_forgets_old_days_but_by_model_keeps_everything() {
        let (dir, files) = temp_files();
        let mut state = BufferState::new();
        state.by_source = one("2020-01-01", "main:old", 1);
        merge_nested(&mut state.by_source, &one(&today_key(), "main:new", 1));
        state.by_model = one("2020-01-01", "p/old", 1);
        merge_nested(&mut state.by_model, &one(&today_key(), "p/new", 1));
        flush_to(&mut state, &files, "test").await;

        let by_source = read_all_by_source_from(&files.by_source).await;
        assert!(!by_source.contains_key("2020-01-01"), "by-source keeps a rolling window");
        assert!(by_source.contains_key(&today_key()));
        let by_model = read_all_by_source_from(&files.by_model).await;
        assert!(by_model.contains_key("2020-01-01"), "by-model is all time");
        assert_eq!(fold_by_model(&by_model, None).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failed_write_keeps_the_buffer_for_the_next_flush() {
        let (dir, mut files) = temp_files();
        // A folder where the file should be: the rename cannot succeed.
        files.days = dir.join("blocked");
        std::fs::create_dir_all(files.days.join("inner")).unwrap();
        let mut state = BufferState::new();
        state.days.entry("2026-11-01".into()).or_default().accumulate(&usage(1, 1), Some(0.0));
        flush_to(&mut state, &files, "test").await;
        assert!(!state.days.is_empty(), "nothing is dropped when the write fails");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
