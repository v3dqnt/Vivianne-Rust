//! Running a `shell` command: in the foreground, as a background job, or in the foreground until its timeout and a
//! job after that.
//!
//! Port of the job half of `@deepseek-ai/dsh-tool-bash` (`index.ts`, `background.ts`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use vivianne_sandbox::confine;
use vivianne_sandbox::output::{Collected, Spill};
use vivianne_sandbox::policy::{self, Mode};
use vivianne_sandbox::spawn::{self, Handle, Output, Spec};

use super::{MAX_OUTPUT_BYTES, SimpleOutput, ToolError, truncate};
use crate::jobs::{
    JobChannel, JobHandle, JobHooks, JobOutcome, JobOutputSource, JobSourceRead, JobSpec,
    JobStatus, Jobs,
};

/// A job's whole stream is kept on disk up to this size, so what falls out of memory can still be read.
const SPILL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Command<'a> {
    pub command: &'a str,
    pub argv: Vec<String>,
    pub start: PathBuf,
    pub mode: Mode,
    pub denial_signatures: &'static [&'static str],
}

impl Command<'_> {
    fn spec(&self, spill: Option<Spill>) -> Spec {
        let mut spec = Spec::new(self.argv.clone(), self.start.clone());
        // One stream each; the collector keeps the end, where errors and results are.
        let keep = || Output::Collect {
            max_bytes: MAX_OUTPUT_BYTES,
            spill: spill.clone(),
        };
        spec.stdout = keep();
        spec.stderr = keep();
        spec
    }

    fn result(
        &self,
        exit_code: Option<i32>,
        stdout: Option<Collected>,
        stderr: Option<Collected>,
    ) -> SimpleOutput {
        let mut text = String::new();
        let mut dropped = false;
        let mut kept = Vec::new();
        for stream in [&stdout, &stderr].into_iter().flatten() {
            dropped |= stream.truncated;
            text.push_str(&stream.text);
            kept.extend(
                stream
                    .spill_path
                    .iter()
                    .map(|path| path.display().to_string()),
            );
        }
        if dropped {
            let note = if kept.is_empty() {
                "[...earlier output dropped]\n".to_owned()
            } else {
                format!(
                    "[...earlier output dropped; full output: {}]\n",
                    kept.join(", ")
                )
            };
            text.insert_str(0, &note);
        }
        let mut text = format!("exit {}\n{}", exit_code.unwrap_or(-1), truncate(text));
        let errors = stderr.map(|s| s.text).unwrap_or_default();
        if confine::denied(exit_code, &errors, self.denial_signatures) {
            text.push_str(&format!(
                "\n{}\n{}",
                policy::denial_marker(self.mode),
                policy::escalation_hint("command")
            ));
        }
        SimpleOutput {
            text,
            details: Some(json!({"command": self.command, "exit_code": exit_code})),
        }
    }
}

fn timed_out(limit: Duration) -> ToolError {
    ToolError::Failed(format!("timed out after {}s", limit.as_secs()))
}

/// The command with a deadline that kills it.
pub(super) async fn foreground(
    call: Command<'_>,
    limit: Duration,
) -> Result<SimpleOutput, ToolError> {
    let mut handle = spawn::spawn(call.spec(None)).map_err(|e| ToolError::Failed(e.to_string()))?;
    // Dropping the handle (a cancelled turn) ends the command and everything it started.
    if !handle.wait_for_exit(limit).await {
        handle.terminate();
        handle.wait_for_exit(Duration::from_secs(5)).await;
        return Err(timed_out(limit));
    }
    let exit_code = handle.done().await.exit_code;
    let (stdout, stderr) = handle.collected();
    Ok(call.result(exit_code, stdout, stderr))
}

/// What the job and the call that started it both look at.
#[derive(Default)]
struct Process {
    handle: Option<Handle>,
    not_started: Option<String>,
}

type Slot = Arc<Mutex<Process>>;

fn lock(slot: &Slot) -> MutexGuard<'_, Process> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Stream {
    slot: Slot,
    errors: bool,
}

impl JobOutputSource for Stream {
    fn channel(&self) -> Option<JobChannel> {
        Some(if self.errors {
            JobChannel::Stderr
        } else {
            JobChannel::Stdout
        })
    }

    fn read(&self, from_byte: u64) -> JobSourceRead {
        let process = lock(&self.slot);
        let read = process.handle.as_ref().and_then(|handle| {
            if self.errors {
                handle.read_stderr(from_byte)
            } else {
                handle.read_stdout(from_byte)
            }
        });
        read.map_or_else(
            || JobSourceRead {
                next_offset: from_byte,
                ..JobSourceRead::default()
            },
            |read| JobSourceRead {
                text: read.text,
                next_offset: read.next_offset,
                lossy: read.lossy,
                spill_path: read.spill_path.map(|path| path.display().to_string()),
            },
        )
    }
}

/// The job that runs `call`: it spawns when the registry admits it, and its kill ends the whole process tree.
fn job(call: &Command<'_>, slot: &Slot) -> JobSpec {
    let dir = crate::results::dir();
    // Without the folder there is no spill file; the output kept in memory is unaffected.
    let spill = std::fs::create_dir_all(&dir).ok().map(|()| Spill {
        max_bytes: SPILL_BYTES,
        dir,
    });
    let mut spec = call.spec(spill);
    let stop = CancellationToken::new();
    spec.cancel = Some(stop.clone());
    let (mode, signatures) = (call.mode, call.denial_signatures);
    let source = |errors| {
        Box::new(Stream {
            slot: Arc::clone(slot),
            errors,
        }) as Box<dyn JobOutputSource>
    };
    let into = Arc::clone(slot);
    let run = move |_: JobHandle| {
        let cancel = {
            let stop = stop.clone();
            Arc::new(move |_: Option<&str>| stop.cancel())
        };
        let done: futures::future::BoxFuture<'static, JobOutcome> = match spawn::spawn(spec) {
            Ok(handle) => {
                let exited = handle.exited();
                lock(&into).handle = Some(handle);
                Box::pin(async move {
                    let ended = exited.await;
                    let killed = stop.is_cancelled();
                    let mut detail = match (killed, ended.signal, ended.exit_code) {
                        (true, Some(signal), _) => format!("signal: {signal}"),
                        (true, None, _) => "killed before exit".to_owned(),
                        (false, _, code) => format!("exit code: {}", code.unwrap_or(-1)),
                    };
                    let errors = lock(&into)
                        .handle
                        .as_ref()
                        .and_then(|handle| handle.collected().1)
                        .map(|stream| stream.text)
                        .unwrap_or_default();
                    if !killed && confine::denied(ended.exit_code, &errors, signatures) {
                        detail.push_str(&format!(
                            "; {} {}",
                            policy::denial_marker(mode),
                            policy::escalation_hint("command")
                        ));
                    }
                    JobOutcome {
                        status: if killed {
                            JobStatus::Killed
                        } else {
                            JobStatus::Completed
                        },
                        detail: Some(detail),
                        result: None,
                    }
                })
            }
            Err(error) => {
                let why = error.to_string();
                lock(&into).not_started = Some(why.clone());
                Box::pin(async move { JobOutcome::failed(why) })
            }
        };
        JobHooks { done, cancel }
    };
    JobSpec {
        kind: "bash".to_owned(),
        label: call.command.to_owned(),
        owner: None,
        output_limit_bytes: None,
        output: vec![source(false), source(true)],
        run: Box::new(run),
    }
}

/// A turn stopped while its command was still in the foreground takes the command with it.
struct StopWithCall<'a> {
    jobs: &'a Jobs,
    id: &'a str,
    armed: bool,
}

impl Drop for StopWithCall<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.jobs.mark_killed(self.id);
            let _ = self
                .jobs
                .registry()
                .kill(self.id, None, Some("tool call aborted"));
        }
    }
}

pub(super) async fn run(
    jobs: &Jobs,
    call: Command<'_>,
    limit: Duration,
    background: bool,
) -> Result<SimpleOutput, ToolError> {
    let slot = Slot::default();
    let failed = |e: crate::jobs::JobError| ToolError::Failed(e.to_string());
    if background {
        // A command that is nothing but empty cannot label a job.
        let id = jobs.registry().start(job(&call, &slot)).map_err(failed)?;
        if let Some(why) = lock(&slot).not_started.clone() {
            return Err(ToolError::Failed(why));
        }
        return Ok(SimpleOutput {
            text: format!("started background job {id}"),
            details: Some(json!({"command": call.command, "job_id": id})),
        });
    }
    let (id, waiting) = match jobs.registry().start_waited(job(&call, &slot)) {
        Ok(started) => started,
        // No room for another job: the command still runs, under the deadline kill.
        Err(error) => {
            log::warn!("shell: not registered as a job, running in the foreground: {error}");
            return foreground(call, limit).await;
        }
    };
    let mut stop = StopWithCall {
        jobs,
        id: &id,
        armed: true,
    };
    let view = waiting.wait(limit).await;
    stop.armed = false;
    if !view.status.is_terminal() {
        // Still going: it stays as a job, and what it has said so far is read now so `job_output` carries on from it.
        let read = jobs.registry().read(&id, None).map_err(failed)?;
        let mut text = crate::jobs::delta(
            &read.chunks,
            read.lossy,
            read.job.output.spill_paths.as_deref(),
        );
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!(
            "[still running after {}s; moved to background job {id}]",
            limit.as_secs()
        ));
        return Ok(SimpleOutput {
            text: truncate(text),
            details: Some(json!({"command": call.command, "job_id": id})),
        });
    }
    // The model never saw the id, so the record goes with this result.
    jobs.registry().remove(&id, None).map_err(failed)?;
    let process = lock(&slot);
    if let Some(why) = &process.not_started {
        return Err(ToolError::Failed(why.clone()));
    }
    let Some(handle) = &process.handle else {
        return Err(ToolError::Failed(
            view.detail
                .as_deref()
                .map_or_else(|| crate::jobs::status_line(&view), str::to_owned),
        ));
    };
    let exit_code = handle.outcome().and_then(|ended| ended.exit_code);
    let (stdout, stderr) = handle.collected();
    Ok(call.result(exit_code, stdout, stderr))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::{Shell, Workspace};
    use crate::builtin::Tool;
    use crate::jobs::{JobOutput, JobStatus, Jobs};

    fn shell(jobs: &Jobs) -> Shell {
        Shell::new(Workspace::new(&std::env::temp_dir())).with_jobs(jobs.clone())
    }

    async fn output(jobs: &Jobs, id: &str) -> String {
        JobOutput(jobs.clone())
            .execute(&json!({"job_id": id, "wait": true}), "")
            .await
            .content
    }

    fn id_of(details: Option<Value>) -> String {
        details.expect("details")["job_id"]
            .as_str()
            .expect("a job")
            .to_owned()
    }

    #[tokio::test]
    async fn a_quick_command_answers_as_before_and_leaves_no_job() {
        let jobs = Jobs::default();
        let out = shell(&jobs)
            .execute(&json!({"command": "echo hello"}), "")
            .await;
        assert!(
            out.content.starts_with("exit 0") && out.content.contains("hello"),
            "{}",
            out.content
        );
        assert!(jobs.registry().list(None).is_empty());
    }

    #[tokio::test]
    async fn a_background_command_returns_at_once_and_its_output_is_collected_later() {
        let jobs = Jobs::default();
        let out = shell(&jobs)
            .execute(
                &json!({"command": "echo later", "run_in_background": true}),
                "",
            )
            .await;
        assert!(
            out.content.starts_with("started background job bash-"),
            "{}",
            out.content
        );
        let read = output(&jobs, &id_of(out.details)).await;
        assert!(
            read.contains("later") && read.ends_with("[status: completed, exit code: 0]"),
            "{read}"
        );
    }

    #[tokio::test]
    async fn a_command_that_outlasts_its_timeout_goes_on_as_a_job() {
        let jobs = Jobs::default();
        let command = if cfg!(windows) {
            "echo first; Start-Sleep -Seconds 8; echo second"
        } else {
            "echo first; sleep 8; echo second"
        };
        let out = shell(&jobs)
            .execute(&json!({"command": command, "timeout_secs": 5}), "")
            .await;
        assert!(
            out.content.contains("first") && out.content.contains("moved to background job"),
            "{}",
            out.content
        );
        let read = output(&jobs, &id_of(out.details)).await;
        assert!(read.contains("second") && !read.contains("first"), "{read}");
    }

    #[tokio::test]
    async fn a_turn_stopped_during_a_command_ends_it() {
        let jobs = Jobs::default();
        let command = if cfg!(windows) {
            "Start-Sleep -Seconds 60"
        } else {
            "sleep 60"
        };
        let (tool, args) = (shell(&jobs), json!({"command": command}));
        // Dropping the call's future is how a turn is stopped.
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(1500),
                tool.execute(&args, "")
            )
            .await
            .is_err()
        );
        let id = jobs.registry().list(None)[0].id.clone();
        let view = jobs
            .registry()
            .wait(&id, std::time::Duration::from_secs(10), None)
            .await
            .expect("known");
        assert_eq!(view.status, JobStatus::Killed, "{:?}", view.detail);
    }
}
