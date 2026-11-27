//! Starting a program and owning it until it and everything it started are gone. Command wording, deadlines and what
//! output means belong to the caller; this runs an argument list in a folder with a scrubbed environment.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::env::child_env;
use crate::output::{Collected, Collector, Spill};
use crate::tree::{self, Tree};

/// How long output is still read after the process ends, in case something it started holds the pipe.
const DRAIN: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Input {
    #[default]
    Ignore,
    Pipe,
    /// Written, then closed.
    Data(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub enum Output {
    Pipe,
    Inherit,
    /// Kept: the last `max_bytes` in memory, and optionally the whole stream in a spill file.
    Collect {
        max_bytes: usize,
        spill: Option<Spill>,
    },
    #[default]
    Ignore,
}

#[derive(Debug, Clone)]
pub struct Spec {
    /// The program and its arguments; the program is looked up on the child's `PATH`.
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    pub stdin: Input,
    pub stdout: Output,
    pub stderr: Output,
    /// Between being asked to stop and being killed (Unix; Windows ends the tree at once).
    pub grace: Duration,
    /// Cancelling it terminates the process.
    pub cancel: Option<CancellationToken>,
    /// Added after the scrub; `None` removes a variable.
    pub env: BTreeMap<OsString, Option<OsString>>,
}

impl Spec {
    #[must_use]
    pub fn new(
        argv: impl IntoIterator<Item = impl Into<OsString>>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: cwd.into(),
            stdin: Input::Ignore,
            stdout: Output::Ignore,
            stderr: Output::Ignore,
            grace: Duration::from_secs(3),
            cancel: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("there is no program to run")]
    Empty,
    #[error("`{0}` was not found")]
    NotFound(String),
    #[error("`{}` is not a folder", .0.display())]
    NotAFolder(PathBuf),
    #[error("could not start: {0}")]
    Io(#[from] std::io::Error),
}

/// How a process ended. `exit_code` is none when a signal ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

type Shared = Arc<Mutex<Collector>>;

/// A running (or finished) process. Dropping it terminates it and everything it started.
#[derive(Debug)]
pub struct Handle {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    collected_out: Option<Shared>,
    collected_err: Option<Shared>,
    stop: CancellationToken,
    done: watch::Receiver<Option<Outcome>>,
}

/// Where `program` is, as the child would find it: an absolute or relative path is checked, a name is searched on the
/// `PATH` the child gets.
pub fn find_program(
    program: &OsString,
    env: &BTreeMap<OsString, OsString>,
    cwd: &std::path::Path,
) -> Result<PathBuf, SpawnError> {
    let missing = || SpawnError::NotFound(program.to_string_lossy().into_owned());
    let path = env
        .iter()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("path"))
        .map(|(_, v)| v.clone());
    which::which_in(program, path, cwd).map_err(|_| missing())
}

/// Starts the program.
pub fn spawn(spec: Spec) -> Result<Handle, SpawnError> {
    let Some(program) = spec.argv.first() else {
        return Err(SpawnError::Empty);
    };
    if !spec.cwd.is_dir() {
        return Err(SpawnError::NotAFolder(spec.cwd));
    }
    let env = child_env(&spec.env);
    let found = find_program(program, &env, &spec.cwd)?;
    let mut command = Command::new(found);
    command
        .args(&spec.argv[1..])
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&env)
        .stdin(match spec.stdin {
            Input::Ignore => Stdio::null(),
            Input::Pipe | Input::Data(_) => Stdio::piped(),
        })
        .stdout(stdio(&spec.stdout))
        .stderr(stdio(&spec.stderr))
        .kill_on_drop(true);
    tree::prepare(&mut command);
    let mut child = command.spawn()?;
    // Without ownership the tree could not be ended, so a process that cannot be adopted is not left running.
    let tree = match tree::adopt(&child) {
        Ok(tree) => tree,
        Err(e) => {
            let _ = child.start_kill();
            return Err(e.into());
        }
    };

    let mut stdin = child.stdin.take();
    if let Input::Data(data) = spec.stdin
        && let Some(mut pipe) = stdin.take()
    {
        tokio::spawn(async move {
            // A program that does not read its input closes the pipe; that is not an error.
            let _ = pipe.write_all(&data).await;
            let _ = pipe.shutdown().await;
        });
    }
    let (stdout, collected_out, out_task) =
        take_output(child.stdout.take(), &spec.stdout, "stdout");
    let (stderr, collected_err, err_task) =
        take_output(child.stderr.take(), &spec.stderr, "stderr");

    let stop = spec
        .cancel
        .as_ref()
        .map_or_else(CancellationToken::new, CancellationToken::child_token);
    let (finished, done) = watch::channel(None);
    tokio::spawn(supervise(
        child,
        tree,
        stop.clone(),
        spec.grace,
        [out_task, err_task],
        finished,
    ));
    Ok(Handle {
        stdin,
        stdout,
        stderr,
        collected_out,
        collected_err,
        stop,
        done,
    })
}

fn stdio(mode: &Output) -> Stdio {
    match mode {
        Output::Pipe | Output::Collect { .. } => Stdio::piped(),
        Output::Inherit => Stdio::inherit(),
        Output::Ignore => Stdio::null(),
    }
}

type Pumped = Option<tokio::task::JoinHandle<()>>;

/// Hands a pipe to the caller, or reads it into a collector.
fn take_output<R: AsyncRead + Unpin + Send + 'static>(
    pipe: Option<R>,
    mode: &Output,
    label: &'static str,
) -> (Option<R>, Option<Shared>, Pumped) {
    match (mode, pipe) {
        (Output::Collect { max_bytes, spill }, Some(mut pipe)) => {
            let shared = Arc::new(Mutex::new(Collector::new(*max_bytes, label, spill.clone())));
            let into = Arc::clone(&shared);
            let task = tokio::spawn(async move {
                let mut chunk = vec![0_u8; 16 * 1024];
                while let Ok(n @ 1..) = pipe.read(&mut chunk).await {
                    into.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(&chunk[..n]);
                }
            });
            (None, Some(shared), Some(task))
        }
        (_, pipe) => (pipe, None, None),
    }
}

async fn supervise(
    mut child: Child,
    tree: Tree,
    stop: CancellationToken,
    grace: Duration,
    pumps: [Pumped; 2],
    finished: watch::Sender<Option<Outcome>>,
) {
    let status = tokio::select! {
        status = child.wait() => status,
        () = stop.cancelled() => {
            tree.ask_to_stop();
            if cfg!(windows) {
                tree.kill();
            }
            if let Ok(status) = tokio::time::timeout(grace, child.wait()).await {
                status
            } else {
                tree.kill();
                child.wait().await
            }
        }
    };
    let mut pumps: Vec<_> = pumps.into_iter().flatten().collect();
    let drained = tokio::time::timeout(DRAIN, async {
        for pump in &mut pumps {
            let _ = pump.await;
        }
    })
    .await;
    // The run is over: whatever it left behind goes, which also closes pipes it was holding open.
    tree.kill();
    if drained.is_err() {
        for pump in &mut pumps {
            if tokio::time::timeout(DRAIN, &mut *pump).await.is_err() {
                pump.abort();
            }
        }
    }
    let outcome = match status {
        Ok(status) => {
            #[cfg(unix)]
            let signal = std::os::unix::process::ExitStatusExt::signal(&status);
            #[cfg(not(unix))]
            let signal = None;
            Outcome {
                exit_code: status.code(),
                signal,
            }
        }
        Err(_) => Outcome {
            exit_code: None,
            signal: None,
        },
    };
    let _ = finished.send(Some(outcome));
}

fn take(shared: Option<&Shared>) -> Option<Collected> {
    shared.map(|c| c.lock().unwrap_or_else(PoisonError::into_inner).finish())
}

impl Handle {
    /// Asks the process and everything it started to stop, and kills them if they do not.
    pub fn terminate(&self) {
        self.stop.cancel();
    }

    /// How it ended, once it has.
    #[must_use]
    pub fn outcome(&self) -> Option<Outcome> {
        *self.done.borrow()
    }

    /// Waits up to `limit` for the process to end; `false` if it has not.
    pub async fn wait_for_exit(&mut self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.done.wait_for(Option::is_some))
            .await
            .is_ok_and(|waited| waited.is_ok())
    }

    /// Waits for it to end.
    pub async fn done(&mut self) -> Outcome {
        match self.done.wait_for(Option::is_some).await {
            Ok(outcome) => outcome.unwrap_or(Outcome {
                exit_code: None,
                signal: None,
            }),
            Err(_) => Outcome {
                exit_code: None,
                signal: None,
            },
        }
    }

    /// Collected standard output since `from`; `None` when it is not collected.
    #[must_use]
    pub fn read_stdout(&self, from: u64) -> Option<crate::output::Read> {
        self.collected_out.as_ref().map(|c| {
            c.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .read_from(from)
        })
    }

    #[must_use]
    pub fn read_stderr(&self, from: u64) -> Option<crate::output::Read> {
        self.collected_err.as_ref().map(|c| {
            c.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .read_from(from)
        })
    }

    /// The final collected output; call after [`Handle::done`].
    #[must_use]
    pub fn collected(&self) -> (Option<Collected>, Option<Collected>) {
        (
            take(self.collected_out.as_ref()),
            take(self.collected_err.as_ref()),
        )
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // A finished process has nothing to stop; a running one is not left without an owner.
        if self.done.borrow().is_none() {
            self.stop.cancel();
        }
    }
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
