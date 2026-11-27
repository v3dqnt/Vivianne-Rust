//! A subagent that is a coding agent (Claude Code, Codex, Gemini CLI and the rest of the Agent Client Protocol
//! registry) run in its own process.
//!
//! Port of `@deepseek-ai/dsh-subagent-acp`. Gaps: `docs/subagent-gaps.md`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use vivianne_providers::acp::{
    Acp, AcpEvent, DELEGATED, Outcome, PromptPart, TurnEnd, TurnRequest,
};

use super::metering;
use super::subagents::{self, Handle, Status};

/// How long a stopped agent gets to end its turn before it is dropped, process and all.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// One piece of work for a coding agent.
#[derive(Debug, Clone)]
pub struct CodingJob {
    /// The agent's id in the registry.
    pub agent: String,
    pub folder: PathBuf,
    pub prompt: String,
    /// It may look and report, not change anything.
    pub read_only: bool,
}

/// Runs a job on a coding agent. A trait so a test needs no agent installed.
#[async_trait]
pub trait CodingAgents: Send + Sync {
    async fn run(
        &self,
        thread: &str,
        job: &CodingJob,
        events: &mpsc::UnboundedSender<AcpEvent>,
        cancel: &CancellationToken,
    ) -> Result<TurnEnd, String>;
}

/// The agents on this machine. Each job has a connection of its own, so its process ends with it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Installed;

#[async_trait]
impl CodingAgents for Installed {
    async fn run(
        &self,
        thread: &str,
        job: &CodingJob,
        events: &mpsc::UnboundedSender<AcpEvent>,
        cancel: &CancellationToken,
    ) -> Result<TurnEnd, String> {
        let prompt = [PromptPart::Text(job.prompt.clone())];
        // No earlier conversation, none of the person's servers or Vivianne's tools, and never bypass: every request
        // to act comes back to be answered.
        let request = TurnRequest {
            prompt: &prompt,
            earlier: &[],
            plan_mode: job.read_only,
            bypass: false,
            mcp_servers: &[],
            own: None,
        };
        let folder = job.folder.display().to_string();
        Acp::new()
            .run_turn(thread, &job.agent, &folder, &request, events, cancel)
            .await
    }
}

/// How a turn's end settles the subagent. A turn that did not finish keeps what was said, marked as partial.
fn status_of(end: Result<TurnEnd, String>) -> Status {
    match end {
        Err(why) => Status::Failed(why),
        Ok(TurnEnd {
            outcome: Outcome::Finished,
            reply,
        }) => Status::Completed(subagents::cut(reply.trim())),
        Ok(TurnEnd {
            outcome: Outcome::Stopped,
            ..
        }) => Status::Cancelled(None),
        Ok(TurnEnd {
            outcome: Outcome::Failed(why),
            reply,
        }) if reply.trim().is_empty() => Status::Failed(why),
        Ok(TurnEnd {
            outcome: Outcome::Failed(why),
            reply,
        }) => Status::Failed(format!(
            "{why}\n\nWhat it had said, unfinished:\n{}",
            subagents::cut(reply.trim())
        )),
    }
}

/// Runs the job to its end, answering what the agent asks: yes only when it may change things.
pub(super) async fn drive(agents: Arc<dyn CodingAgents>, job: CodingJob, handle: Arc<Handle>) {
    let (events, mut seen) = mpsc::unbounded_channel();
    let allow = !job.read_only;
    // Read until the channel closes: a request left unread would keep the agent waiting for ever.
    let answer = async {
        while let Some(event) = seen.recv().await {
            match event {
                AcpEvent::Permission(request) => request.answer(allow),
                AcpEvent::ToolStart { .. } => handle.count_tool_call(),
                AcpEvent::Usage(spent) => metering::record(&handle.thread, &spent).await,
                _ => {}
            }
        }
    };
    let thread = format!("{DELEGATED}task-{}", handle.id);
    let work = async {
        let overdue = async {
            handle.cancel.cancelled().await;
            tokio::time::sleep(STOP_GRACE).await;
        };
        let status = tokio::select! {
            end = agents.run(&thread, &job, &events, &handle.cancel) => status_of(end),
            () = overdue => Status::Cancelled(None),
        };
        drop(events);
        status
    };
    let (status, ()) = tokio::join!(work, answer);
    handle.settle(status);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use vivianne_core::Mode;

    use super::*;
    use crate::orchestration::Caller;

    /// An agent that asks nothing, says one thing and ends as told.
    struct Fake {
        end: Outcome,
        seen: Mutex<Vec<(String, CodingJob)>>,
    }

    #[async_trait]
    impl CodingAgents for Fake {
        async fn run(
            &self,
            thread: &str,
            job: &CodingJob,
            events: &mpsc::UnboundedSender<AcpEvent>,
            _cancel: &CancellationToken,
        ) -> Result<TurnEnd, String> {
            self.seen
                .lock()
                .unwrap()
                .push((thread.to_owned(), job.clone()));
            let _ = events.send(AcpEvent::ToolStart {
                id: "1".to_owned(),
                name: "read".to_owned(),
                args: json!({}),
            });
            Ok(TurnEnd {
                outcome: self.end.clone(),
                reply: "the answer".to_owned(),
            })
        }
    }

    fn caller(thread: &str) -> Caller {
        Caller {
            thread: thread.to_owned(),
            mode: Mode::Auto,
            folder: std::env::temp_dir(),
            model: None,
            provider: None,
            confined: false,
            depth: 0,
        }
    }

    async fn run_with(thread: &str, end: Outcome) -> (Arc<Handle>, Arc<Fake>) {
        let fake = Arc::new(Fake {
            end,
            seen: Mutex::default(),
        });
        let job = CodingJob {
            agent: "claude-acp".to_owned(),
            folder: std::env::temp_dir(),
            prompt: "look".to_owned(),
            read_only: false,
        };
        let handle = subagents::start_coding(
            fake.clone(),
            &caller(thread),
            "agent-1".to_owned(),
            "looks".to_owned(),
            job,
        )
        .expect("started");
        handle.wait(Duration::from_secs(5)).await;
        (handle, fake)
    }

    #[tokio::test]
    async fn a_finished_turn_completes_under_a_delegated_thread() {
        let (handle, fake) = run_with("coding-done", Outcome::Finished).await;
        assert_eq!(handle.status(), Status::Completed("the answer".to_owned()));
        assert!(
            handle.summary().contains("1 tool calls"),
            "{}",
            handle.summary()
        );
        let seen = fake.seen.lock().unwrap();
        assert!(seen[0].0.starts_with(DELEGATED), "{}", seen[0].0);
    }

    #[tokio::test]
    async fn a_cut_off_answer_is_a_failure_that_keeps_what_was_said() {
        let why = Outcome::Failed("it ran out of room".to_owned());
        let (handle, _) = run_with("coding-cut", why).await;
        let Status::Failed(said) = handle.status() else {
            panic!("{:?}", handle.status());
        };
        assert!(
            said.starts_with("it ran out of room") && said.contains("the answer"),
            "{said}"
        );
        let (stopped, _) = run_with("coding-stopped", Outcome::Stopped).await;
        assert_eq!(stopped.status(), Status::Cancelled(None));
    }
}
