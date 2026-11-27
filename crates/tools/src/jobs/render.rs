//! What the model reads about a job: status lines, output deltas and the completion notice.
//!
//! Port of `@deepseek-ai/dsh-tool-jobs/render.ts` and its notice text.

use super::registry::Registry;
use super::types::{JobChannel, JobChunk, JobView};

fn detail(job: &JobView) -> Option<&str> {
    job.progress.as_deref().or(job.detail.as_deref())
}

pub(crate) fn status_line(job: &JobView) -> String {
    detail(job).map_or_else(
        || format!("[status: {}]", job.status),
        |detail| format!("[status: {}, {detail}]", job.status),
    )
}

/// Standard output in order, then one `[stderr]` section; the `log` channel is not the model's.
pub(crate) fn delta(chunks: &[JobChunk], lossy: bool, spill_paths: Option<&[String]>) -> String {
    let visible: Vec<&JobChunk> = chunks
        .iter()
        .filter(|chunk| chunk.channel != Some(JobChannel::Log))
        .collect();
    let join = |errors: bool| -> String {
        visible
            .iter()
            .filter(|chunk| (chunk.channel == Some(JobChannel::Stderr)) == errors)
            .map(|chunk| chunk.text.as_str())
            .collect()
    };
    let (mut body, errors) = (join(false), join(true));
    if !errors.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(&errors);
    }
    if !lossy && !visible.iter().any(|chunk| chunk.gap_before) {
        return body;
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    let paths = match spill_paths {
        Some(paths) if !paths.is_empty() => paths.join(", "),
        _ => "(unavailable)".to_owned(),
    };
    body.push_str(&format!(
        "[some output was dropped from memory; full output: {paths}]"
    ));
    body
}

/// Keeps the end of `text` within the job's own cap, when it has one.
pub(crate) fn fit(text: String, limit: Option<usize>) -> String {
    let Some(limit) = limit.filter(|limit| text.len() > *limit) else {
        return text;
    };
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

pub(super) fn notice(registry: &Registry, job: &JobView, inline_bytes: u64) -> String {
    let head = format!(
        "background job {} ({}: {}) finished {}.",
        job.id,
        job.kind,
        job.label,
        status_line(job)
    );
    // Short enough to quote whole, which also counts as read; a longer one is left unread.
    let quoted = (job.output.total <= inline_bytes)
        .then(|| registry.read(&job.id, job.owner.as_deref()).ok())
        .flatten();
    let text = match quoted {
        Some(read) => {
            let mut body = delta(
                &read.chunks,
                read.lossy,
                read.job.output.spill_paths.as_deref(),
            );
            if let Some(result) = read.result {
                body.push_str(&result);
            }
            if body.is_empty() {
                format!("{head} No new output.")
            } else {
                format!(
                    "{head} Output:
{body}"
                )
            }
        }
        None => format!("{head} Read its output with job_output."),
    };
    fit(text, job.output_limit_bytes)
}
