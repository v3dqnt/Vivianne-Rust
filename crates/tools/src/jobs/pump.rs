//! Drains a job's pull sources into its ring while the producer runs, and once more after it finishes.
//!
//! Port of `@deepseek-ai/dsh-jobs-local/pump.ts`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::job::TrackedJob;
use super::types::{JobAppendOptions, JobOutputSource, Writer};

struct Cursor {
    source: Box<dyn JobOutputSource>,
    at: u64,
    failed: bool,
}

fn drain(job: &TrackedJob, cursors: &mut [Cursor]) {
    for (index, cursor) in cursors.iter_mut().enumerate() {
        if cursor.failed {
            continue;
        }
        // A source that fails stops here; the others and the job go on.
        let Ok(read) = catch_unwind(AssertUnwindSafe(|| cursor.source.read(cursor.at))) else {
            log::warn!(
                "jobs: output source for {} failed; its stream stops here",
                job.id()
            );
            cursor.failed = true;
            continue;
        };
        cursor.at = read.next_offset;
        job.spill(index, read.spill_path);
        let options = JobAppendOptions {
            channel: cursor.source.channel(),
            gap_before: read.lossy,
        };
        job.append(&read.text, options, Writer::Pump);
    }
}

/// Runs until `finished` turns true (or its sender goes), then drains one last time.
pub(super) async fn run(
    job: Arc<TrackedJob>,
    sources: Vec<Box<dyn JobOutputSource>>,
    every: Duration,
    mut finished: watch::Receiver<bool>,
) {
    let mut cursors: Vec<Cursor> = sources
        .into_iter()
        .map(|source| Cursor {
            source,
            at: 0,
            failed: false,
        })
        .collect();
    loop {
        drain(&job, &mut cursors);
        let over = tokio::time::timeout(every, finished.wait_for(|done| *done)).await;
        if over.is_ok() {
            break;
        }
    }
    drain(&job, &mut cursors);
}
