//! One turn with an agent reached over given pipes rather than started from the registry: for an agent that runs
//! inside this process, as Vivianne's own server does in tests.

use std::collections::HashMap;

use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::conn::{TreeKillOnDrop, handshake, wire};
use super::registry::Harness;
use super::session::{self, DELEGATE};
use super::turn::{self, AcpEvent, TurnEnd, TurnRequest};

/// A thread id starting with this is a delegated job: its session is never saved or resumed.
pub const DELEGATED: &str = DELEGATE;

fn piped() -> Harness {
    Harness {
        id: "pipes".to_owned(),
        name: "The agent".to_owned(),
        description: String::new(),
        icon: None,
        available: true,
        ready: true,
        launcher: String::new(),
        args: Vec::new(),
        env: HashMap::new(),
        package: None,
        extra: Vec::new(),
        managed: true,
    }
}

/// Initializes, opens a session in `cwd` and runs one turn. `thread` should start with [`DELEGATED`].
pub async fn run_over(
    reads: Box<dyn AsyncBufRead + Unpin + Send>,
    writes: Box<dyn AsyncWrite + Unpin + Send>,
    thread: &str,
    cwd: &str,
    request: &TurnRequest<'_>,
    events: &mpsc::UnboundedSender<AcpEvent>,
    cancel: &CancellationToken,
) -> Result<TurnEnd, String> {
    let harness = piped();
    let conn = wire(
        writes,
        reads,
        None,
        TreeKillOnDrop { pid: None },
        harness.name.clone(),
    );
    handshake(&conn, &harness).await?;
    let session = session::open_with(&conn, &harness, None, cwd, &[]).await?;
    Ok(turn::run(&conn, &harness, &session, thread, request, events, cancel).await)
}
