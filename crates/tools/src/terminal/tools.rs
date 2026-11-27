//! `terminal_open`, `terminal_send`, `terminal_read`, `terminal_signal`, `terminal_close`, `terminal_list`. Their specs
//! are in `schemas.rs`.
//!
//! Port of `@deepseek-ai/dsh-tool-terminal`.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::session::{Session, Wait};
use super::{Record, Terminals};
use crate::builtin::{Tool, ToolResult};
use crate::jobs::{JobHooks, JobOutcome, JobSpec, JobStatus};

const READ_LINES: u64 = 500;
const MAX_READ_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Open,
    Send,
    Read,
    Signal,
    Close,
    List,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::Open,
        Self::Send,
        Self::Read,
        Self::Signal,
        Self::Close,
        Self::List,
    ];
}

#[derive(Debug)]
pub struct TerminalTool(pub Kind, pub Terminals);

fn status(session: &Session) -> String {
    if !session.exited() {
        return "running".to_owned();
    }
    let code = session
        .exit_code()
        .map_or_else(|| "null".to_owned(), |c| c.to_string());
    format!("exited code={code}")
}

/// The end of `text` within the read limit.
fn bounded(text: String) -> String {
    if text.len() <= MAX_READ_BYTES {
        return text;
    }
    let mut start = text.len() - MAX_READ_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{}\n[output truncated]", &text[start..])
}

fn sent(session: &Session, wait: Wait) -> String {
    let mut view = session.viewport();
    if view.is_empty() {
        view.push_str("(no new output)");
    }
    format!(
        "{}\n[wait: {}]\n[session: {}]",
        bounded(view),
        wait.as_str(),
        status(session)
    )
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .ok_or_else(|| format!("`{key}` must be a string"))
}

fn label(record: &Record) -> String {
    record.name.as_ref().map_or_else(
        || record.id.clone(),
        |name| format!("{} ({name})", record.id),
    )
}

impl TerminalTool {
    async fn open(&self, args: &Value) -> Result<String, String> {
        let kind = text(args, "type")?;
        if kind != "shell" {
            return Err(format!("no terminal backend registered for \"{kind}\""));
        }
        let started = Instant::now();
        let record = self.1.open(args["name"].as_str(), args["cwd"].as_str())?;
        record.session.settle(started).await;
        let motd = record.session.viewport();
        let motd = if motd.is_empty() {
            "(no startup output)"
        } else {
            &motd
        };
        Ok(format!(
            "started terminal session {} [type: shell]\n{motd}",
            label(&record)
        ))
    }

    async fn send(&self, args: &Value) -> Result<String, String> {
        let id = text(args, "sessionId")?;
        let mut input = text(args, "text")?.to_owned();
        let record = self.1.get(id)?;
        let session = record.session;
        if session.exited() {
            return Err(format!("terminal session {id} has exited"));
        }
        let sending = session
            .begin_send()
            .ok_or_else(|| format!("terminal session {id} already has an active send"))?;
        if args["submit"].as_bool() != Some(false) {
            input.push('\r');
        }
        let since = Instant::now();
        if args["run_in_background"].as_bool() != Some(true) {
            session.write(input.into_bytes());
            let wait = session.settle(since).await;
            drop(sending);
            return Ok(sent(&session, wait));
        }
        let stop = CancellationToken::new();
        let job = JobSpec {
            kind: "terminal".to_owned(),
            label: format!("{id}: {}", input.trim_end()),
            owner: None,
            output_limit_bytes: None,
            output: Vec::new(),
            run: Box::new(move |_| {
                let cancel = {
                    let (stop, session) = (stop.clone(), Arc::clone(&session));
                    Arc::new(move |_: Option<&str>| {
                        // Interrupts what is running and gives up the wait.
                        session.write(vec![3]);
                        stop.cancel();
                    })
                };
                session.write(input.into_bytes());
                let done = Box::pin(async move {
                    let _sending = sending;
                    tokio::select! {
                        wait = session.settle(since) => JobOutcome {
                            status: JobStatus::Completed,
                            detail: Some(format!("wait: {}", wait.as_str())),
                            result: Some(sent(&session, wait)),
                        },
                        () = stop.cancelled() => JobOutcome {
                            status: JobStatus::Killed,
                            detail: None,
                            result: Some(session.viewport()),
                        },
                    }
                });
                JobHooks { done, cancel }
            }),
        };
        let id = self
            .1
            .jobs
            .registry()
            .start(job)
            .map_err(|e| e.to_string())?;
        Ok(format!("started background job {id}"))
    }

    fn read(&self, args: &Value) -> Result<String, String> {
        let record = self.1.get(text(args, "sessionId")?)?;
        let offset = args["offset"].as_u64().unwrap_or(0);
        let count = args["count"].as_u64().unwrap_or(READ_LINES);
        let as_size = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
        let (page, total, begin, end) = record.session.read(as_size(offset), as_size(count));
        let page = if page.is_empty() {
            "(no retained output)".to_owned()
        } else {
            bounded(page)
        };
        Ok(format!("{page}\n[lines {begin}-{end} of {total}]"))
    }

    fn signal(&self, args: &Value) -> Result<String, String> {
        let record = self.1.get(text(args, "sessionId")?)?;
        let signal = text(args, "signal")?;
        let key: u8 = match signal {
            "SIGINT" => 3,
            "SIGTSTP" if cfg!(unix) => 26,
            "SIGKILL" => {
                return Err("SIGKILL would take the shell itself; use terminal_close".to_owned());
            }
            "SIGTSTP" | "SIGTERM" | "SIGHUP" => {
                return Err(format!(
                    "{signal} cannot be delivered here; use SIGINT or terminal_close"
                ));
            }
            other => return Err(format!("unknown signal {other}")),
        };
        record.session.write(vec![key]);
        Ok(format!(
            "delivered {signal} to terminal session {}",
            record.id
        ))
    }

    fn list(&self) -> String {
        let lines: Vec<String> = self
            .1
            .list()
            .iter()
            .map(|r| format!("{} [shell] {}", label(r), status(&r.session)))
            .collect();
        if lines.is_empty() {
            "(no terminal sessions)".to_owned()
        } else {
            lines.join("\n")
        }
    }
}

#[async_trait]
impl Tool for TerminalTool {
    fn name(&self) -> &str {
        match self.0 {
            Kind::Open => "terminal_open",
            Kind::Send => "terminal_send",
            Kind::Read => "terminal_read",
            Kind::Signal => "terminal_signal",
            Kind::Close => "terminal_close",
            Kind::List => "terminal_list",
        }
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let outcome = match self.0 {
            Kind::Open => self.open(args).await,
            Kind::Send => self.send(args).await,
            Kind::Read => self.read(args),
            Kind::Signal => self.signal(args),
            Kind::Close => text(args, "sessionId").and_then(|id| {
                self.1.close(id)?;
                Ok(format!("closed terminal session {id}"))
            }),
            Kind::List => Ok(self.list()),
        };
        match outcome {
            Ok(content) => ToolResult {
                content,
                is_error: false,
                details: Some(json!({ "terminal": args["sessionId"] })),
            },
            Err(why) => ToolResult::error(why),
        }
    }
}
