//! The model's three job tools: `job_output`, `job_list`, `job_kill`. Their specs are in `schemas.rs`.
//!
//! Port of `@deepseek-ai/dsh-tool-jobs/index.ts`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::Jobs;
use super::render::{delta, fit, status_line};
use super::types::{JobStatus, KillResult};
use crate::builtin::{Tool, ToolResult};

const DEFAULT_WAIT_MS: u64 = 30_000;
const MAX_WAIT_MS: u64 = 600_000;

fn job_id(args: &Value) -> Result<&str, ToolResult> {
    args["job_id"]
        .as_str()
        .ok_or_else(|| ToolResult::error("`job_id` must be a string"))
}

#[derive(Debug)]
pub struct JobOutput(pub Jobs);

#[async_trait]
impl Tool for JobOutput {
    fn name(&self) -> &'static str {
        "job_output"
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let id = match job_id(args) {
            Ok(id) => id,
            Err(refused) => return refused,
        };
        let registry = self.0.registry();
        if args["wait"].as_bool() == Some(true) {
            let limit = args["timeout_ms"]
                .as_u64()
                .unwrap_or(DEFAULT_WAIT_MS)
                .clamp(1, MAX_WAIT_MS);
            // The person saying something ends the wait, not the job.
            let waited = tokio::select! {
                waited = registry.wait(id, Duration::from_millis(limit), None) => waited,
                () = self.0.until_steered() => registry.get(id, None),
            };
            if let Err(error) = waited {
                return ToolResult::error(error.to_string());
            }
        }
        let read = match registry.read(id, None) {
            Ok(read) => read,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let mut body = delta(
            &read.chunks,
            read.lossy,
            read.job.output.spill_paths.as_deref(),
        );
        if let Some(result) = &read.result {
            if !body.is_empty() && !body.ends_with('\n') {
                body.push('\n');
            }
            body.push_str(result);
        }
        if body.is_empty() {
            body.push_str("(no new output)");
        }
        let status = status_line(&read.job);
        let body = fit(
            body,
            read.job
                .output_limit_bytes
                .map(|limit| limit.saturating_sub(status.len() + 1)),
        );
        ToolResult {
            content: format!("{}\n{status}", body.trim_end_matches('\n')),
            is_error: false,
            details: Some(json!({ "job_id": id, "status": read.job.status })),
        }
    }
}

#[derive(Debug)]
pub struct JobList(pub Jobs);

#[async_trait]
impl Tool for JobList {
    fn name(&self) -> &'static str {
        "job_list"
    }

    async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
        let lines: Vec<String> = self
            .0
            .registry()
            .list(None)
            .iter()
            .map(|job| format!("{} [{}] {} — {}", job.id, job.kind, job.status, job.label))
            .collect();
        if lines.is_empty() {
            ToolResult::ok("(no background jobs)")
        } else {
            ToolResult::ok(lines.join("\n"))
        }
    }
}

#[derive(Debug)]
pub struct JobKill(pub Jobs);

#[async_trait]
impl Tool for JobKill {
    fn name(&self) -> &'static str {
        "job_kill"
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let id = match job_id(args) {
            Ok(id) => id,
            Err(refused) => return refused,
        };
        // Marked first: a job that ends the instant it is asked to must not send a notice.
        self.0.mark_killed(id);
        let registry = self.0.registry();
        match registry.kill(id, None, args["reason"].as_str()) {
            Ok(KillResult::Requested) => {
                ToolResult::ok(format!("requested cancellation of job {id}"))
            }
            Ok(KillResult::AlreadyFinished) => {
                let status = registry.get(id, None).map_or_else(
                    |_| format!("[status: {}]", JobStatus::Completed),
                    |job| status_line(&job),
                );
                ToolResult::ok(format!("job {id} had already finished {status}"))
            }
            Err(error) => ToolResult::error(error.to_string()),
        }
    }
}
