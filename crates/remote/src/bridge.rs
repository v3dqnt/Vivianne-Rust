//! The two ends of the bridge.
//!
//! **On the machine with the server** (`viv remote-client-bridge`): make sure a server runs for the session
//! (starting one, detached, when none does), then copy bytes both ways between the standard streams, which SSH
//! carries, and the server's local socket. With `--idle-timeout-v1` it leaves after a minute without traffic; the
//! server and its runs are untouched.
//!
//! **On the client** ([`open`]): start `ssh ... remote-client-bridge` (or the local bridge for the loopback target),
//! drop anything printed before the ready marker, and do the handshake.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::catalog::SavedMachine;
use crate::protocol::{
    ClientMessage, Negotiated, ServerMessage, accept_welcome, client_hello, read_frame, write_frame,
};
use crate::ssh::{self, LOOPBACK_TARGET, REMOTE_OUTPUT_READY_MARKER};
use crate::{CAP_WANTED, socket};

/// How long the server gets to start listening after it is started.
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(5);
/// With `--idle-timeout-v1`, a bridge without traffic for this long exits.
pub const BRIDGE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
const LOCAL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MARKER_TIMEOUT: Duration = Duration::from_secs(20);

/// Starts this session's server, detached, if none answers. Returns once it does.
pub async fn ensure_server(session: &str) -> Result<(), String> {
    if socket::is_listening(session).await {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--session")
        .arg(session)
        .arg("server")
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW: it outlives this process and its console.
        // CREATE_BREAKAWAY_FROM_JOB too, so a job that ends with its launcher (a terminal's, a service's) does not
        // take the server with it; if the job forbids breakaway the spawn is retried without it.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000 | 0x0100_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // A new process group, so closing the launching shell does not signal it.
        cmd.process_group(0);
    }
    if cmd.spawn().is_err() {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
            return finish_spawn(cmd.spawn(), session).await;
        }
        #[cfg(not(windows))]
        return Err(format!("could not start the server: {first}"));
    }
    finish_spawn(Ok(()), session).await
}

async fn finish_spawn<T>(spawned: std::io::Result<T>, session: &str) -> Result<(), String> {
    spawned.map_err(|e| format!("could not start the server: {e}"))?;
    let deadline = tokio::time::Instant::now() + SERVER_START_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if socket::is_listening(session).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("the server did not start listening in time".to_owned())
}

/// The machine side: forward the standard streams to the server's socket. `idle_timeout` makes it leave when quiet.
pub async fn run_host(session: &str, idle_timeout: bool) -> Result<(), String> {
    ensure_server(session).await?;
    let stream = socket::connect(session).await.map_err(|e| e.to_string())?;
    let (mut from_server, mut to_server) = tokio::io::split(stream);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let last = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let (last_up, last_down) = (last.clone(), last.clone());
    let touch = |at: &std::sync::Mutex<std::time::Instant>| {
        *at.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = std::time::Instant::now();
    };
    let up = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = stdin.read(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 {
                return Ok::<(), String>(());
            }
            touch(&last_up);
            to_server
                .write_all(&buf[..n])
                .await
                .map_err(|e| e.to_string())?;
            to_server.flush().await.map_err(|e| e.to_string())?;
        }
    };
    let down = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = from_server
                .read(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Ok::<(), String>(());
            }
            touch(&last_down);
            stdout
                .write_all(&buf[..n])
                .await
                .map_err(|e| e.to_string())?;
            stdout.flush().await.map_err(|e| e.to_string())?;
        }
    };
    // Either direction ending ends the bridge. The idle exit is checked by the caller's heartbeat: a connected
    // client pings every few seconds, so a minute of silence means nobody is there.
    let idle = async {
        if !idle_timeout {
            std::future::pending::<()>().await;
        }
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let quiet = last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed();
            if quiet >= BRIDGE_IDLE_TIMEOUT {
                return;
            }
        }
    };
    tokio::select! {
        r = up => r,
        r = down => r,
        () = idle => Ok(()),
    }
}

/// An open, handshaken connection to a machine's server.
pub struct Connection {
    pub reader: BufReader<ChildStdout>,
    pub writer: ChildStdin,
    pub negotiated: Negotiated,
    /// Killed when the connection is dropped, so no `ssh` is left behind.
    pub child: Child,
    /// What the child printed on stderr, for the reason a connection failed.
    pub stderr: tokio::task::JoinHandle<String>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Connection")
    }
}

/// The local `viv` that the loopback target runs: next to this program, else on the PATH.
fn local_viv() -> Result<std::path::PathBuf, String> {
    let name = if cfg!(windows) { "viv.exe" } else { "viv" };
    if let Ok(exe) = std::env::current_exe() {
        if exe.file_name().is_some_and(|n| n == name) {
            return Ok(exe);
        }
        if let Some(sibling) = exe.parent().map(|d| d.join(name)).filter(|p| p.is_file()) {
            return Ok(sibling);
        }
    }
    which_in_path(name)
        .ok_or_else(|| "`viv` was not found next to this program or on the PATH".to_owned())
}

fn which_in_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Starts the bridge for `machine`: `ssh` for a real target, the local program for the loopback one. Returns the
/// child and whether its output starts with the ready marker.
fn spawn(machine: &SavedMachine, managed: bool) -> Result<(Child, bool), String> {
    let (mut cmd, marker) = if machine.target == LOOPBACK_TARGET {
        let mut c = tokio::process::Command::new(local_viv()?);
        c.arg("--session")
            .arg(&machine.session)
            .arg("remote-client-bridge");
        (c, false)
    } else {
        let opts = ssh::options(true, managed);
        let windows = crate::meta::get(&machine.id).is_some_and(|m| m.os == "windows");
        let remote = if windows {
            ssh::remote_bridge_command_windows(&machine.session, true)
        } else {
            ssh::remote_bridge_command(&machine.session, true)
        };
        let mut c = tokio::process::Command::new(ssh::program());
        c.args(ssh::args(&opts, &machine.target, Some(&remote)));
        (c, true)
    };
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    // SSH_ASKPASS is cleared so a background connection can never put up a password dialog.
    cmd.env_remove("SSH_ASKPASS")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| format!("could not run ssh: {e}"))?;
    Ok((child, marker))
}

/// Ends a failed attempt: the child is killed and the reason carries what it printed on stderr.
async fn fail(child: &mut Child, stderr: tokio::task::JoinHandle<String>, why: String) -> String {
    let _ = child.start_kill();
    let tail = tokio::time::timeout(Duration::from_millis(500), stderr)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if tail.trim().is_empty() {
        why
    } else {
        format!("{why}: {}", tail.trim())
    }
}

/// Opens a connection to `machine` and does the handshake.
pub async fn open(machine: &SavedMachine, managed: bool) -> Result<Connection, String> {
    let (mut child, marker) = spawn(machine, managed)?;
    let (stdin, stdout, stderr) = (
        child.stdin.take().ok_or("no stdin")?,
        child.stdout.take().ok_or("no stdout")?,
        child.stderr.take().ok_or("no stderr")?,
    );
    let stderr_task = tokio::spawn(async move {
        let mut text = Vec::new();
        let mut limited = stderr.take(ssh::NONINTERACTIVE_STDERR_LIMIT as u64);
        let _ = limited.read_to_end(&mut text).await;
        String::from_utf8_lossy(&text).into_owned()
    });
    let mut reader = BufReader::new(stdout);
    let mut writer = stdin;
    if marker {
        // A login banner or message of the day comes first; skip lines up to the marker.
        let found = tokio::time::timeout(MARKER_TIMEOUT, async {
            let mut line = String::new();
            loop {
                line.clear();
                if reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| e.to_string())?
                    == 0
                {
                    return Err("the connection closed before the remote was ready".to_owned());
                }
                if line.trim() == REMOTE_OUTPUT_READY_MARKER {
                    return Ok(());
                }
            }
        })
        .await;
        match found {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return Err(fail(&mut child, stderr_task, why).await),
            Err(_) => {
                return Err(fail(
                    &mut child,
                    stderr_task,
                    "the remote was not ready in time".to_owned(),
                )
                .await);
            }
        }
    }

    let timeout = if marker {
        REMOTE_HANDSHAKE_TIMEOUT
    } else {
        LOCAL_HANDSHAKE_TIMEOUT
    };
    let hello = ClientMessage::Hello(client_hello(CAP_WANTED));
    if let Err(e) = write_frame(&mut writer, &hello).await {
        return Err(fail(&mut child, stderr_task, e.to_string()).await);
    }
    let welcome =
        match tokio::time::timeout(timeout, read_frame::<_, ServerMessage>(&mut reader)).await {
            Ok(Ok(ServerMessage::Welcome(w))) => w,
            Ok(Ok(_)) => {
                return Err(fail(
                    &mut child,
                    stderr_task,
                    "handshake failed: unexpected first message".to_owned(),
                )
                .await);
            }
            Ok(Err(e)) => {
                return Err(fail(&mut child, stderr_task, format!("handshake failed: {e}")).await);
            }
            Err(_) => {
                return Err(fail(
                    &mut child,
                    stderr_task,
                    "handshake failed: no answer in time".to_owned(),
                )
                .await);
            }
        };
    match accept_welcome(welcome) {
        Ok(negotiated) => Ok(Connection {
            reader,
            writer,
            negotiated,
            child,
            stderr: stderr_task,
        }),
        Err(why) => Err(fail(&mut child, stderr_task, why).await),
    }
}
