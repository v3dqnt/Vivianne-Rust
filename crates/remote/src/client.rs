//! The client's side of every saved machine: one link per machine that connects, keeps itself connected with the
//! supervisor's backoff and the health checks, and publishes what it last learned. The app reads
//! [`Hub::views`]; nothing here blocks the UI, and a machine that is down never delays another.
//!
//! What a link last knew stays visible while it is down (shown dimmed, with input off) until a fresh connection
//! and a fresh snapshot replace it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use crate::bridge::{self, Connection};
use crate::catalog::{Catalog, SavedMachine};
use crate::health::{self, Health};
use crate::protocol::{
    AgentState, CAP_SURFACE_INTEREST, ClientMessage, PaneInfo, RunInfo, ServerMessage, ThreadInfo,
    WireEvent, read_frame, write_frame,
};
use crate::ssh::{self, Failure};
use crate::supervisor::{Status, Supervisor};

/// What the UI shows of one machine.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub machine: SavedMachine,
    pub status: Status,
    /// Why it is not online, in a sentence. Empty when it is.
    pub message: String,
    /// Which kind of attention it needs, when it does.
    pub failure: Option<Failure>,
    pub server_version: String,
    pub threads: Vec<ThreadInfo>,
    pub runs: Vec<RunInfo>,
    /// Run output since the link started, newest last, capped.
    pub activity: Vec<(String, WireEvent)>,
    /// The terminal panes the machine has.
    pub panes: Vec<PaneInfo>,
    /// The last screen of each pane that was in view, by pane id: (lines, cursor row and column).
    pub frames: std::collections::HashMap<String, (Vec<String>, (u16, u16))>,
    /// The last notice the machine sent (a run finished, say).
    pub notice: Option<(String, String)>,
}

impl View {
    /// The short mark shown beside a machine that needs the person: `! auth`, `! error`, or nothing.
    #[must_use]
    pub fn badge(&self) -> &'static str {
        match (self.status, self.failure) {
            (Status::Attention, Some(Failure::Auth | Failure::HostKey)) => "! auth",
            (Status::Attention, _) => "! error",
            _ => "",
        }
    }

    /// What to tell the person, in a sentence.
    #[must_use]
    pub fn status_text(&self) -> String {
        match (self.status, self.failure) {
            (Status::Online, _) => format!("Online, server {}", self.server_version),
            (Status::Connecting, _) => "Connecting…".to_owned(),
            (Status::Reconnecting, _) => "Reconnecting…".to_owned(),
            (Status::Disabled, _) => "Off".to_owned(),
            (Status::Attention, Some(Failure::HostKey)) => {
                format!(
                    "The host key is unknown or changed. Check it with ssh, then run `viv machine reconnect {}`.",
                    self.machine.label
                )
            }
            (Status::Attention, Some(Failure::Auth)) => {
                format!(
                    "Sign-in is needed. Run `viv machine reconnect {}` in a terminal.",
                    self.machine.label
                )
            }
            (Status::Attention, _) => self.message.clone(),
        }
    }

    /// Whether what is shown is old: the link is down, so the list is the last one it saw.
    #[must_use]
    pub fn is_cached(&self) -> bool {
        self.status != Status::Online
    }
}

const ACTIVITY_CAP: usize = 400;

enum Command {
    Send(ClientMessage),
    RetryNow,
    Stop,
}

struct Link {
    view: watch::Receiver<View>,
    commands: mpsc::UnboundedSender<Command>,
    machine: SavedMachine,
}

/// All the links.
#[derive(Default)]
pub struct Hub {
    links: Mutex<HashMap<String, Link>>,
}

static HUB: OnceLock<Hub> = OnceLock::new();
/// Whether new connections should stream (see [`Hub::set_all_in_view`]).
static ALL_IN_VIEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Hub {
    #[must_use]
    pub fn global() -> &'static Hub {
        HUB.get_or_init(Hub::default)
    }

    /// Brings the links in line with the catalog: new machines connect, removed ones disconnect (their remote
    /// work keeps running), changed ones reconnect. Needs a tokio runtime.
    pub fn sync(&self, catalog: &Catalog, handle: &tokio::runtime::Handle) {
        let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        let wanted: Vec<&SavedMachine> = catalog.ssh.iter().collect();
        links.retain(|id, link| {
            let keep = wanted.iter().any(|m| &m.id == id && **m == link.machine);
            if !keep {
                let _ = link.commands.send(Command::Stop);
            }
            keep
        });
        for machine in wanted {
            if !links.contains_key(&machine.id) {
                links.insert(machine.id.clone(), spawn_link(machine.clone(), handle));
            }
        }
    }

    /// What to show, in catalog order is the caller's business: here in label order.
    #[must_use]
    pub fn views(&self) -> Vec<View> {
        let links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        let mut views: Vec<View> = links.values().map(|l| l.view.borrow().clone()).collect();
        views.sort_by(|a, b| a.machine.label.cmp(&b.machine.label));
        views
    }

    /// Reconnects now (`machine reconnect`).
    pub fn retry_now(&self, id: &str) {
        if let Some(link) = self
            .links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            let _ = link.commands.send(Command::RetryNow);
        }
    }

    /// Sends a message to a machine. Nothing is sent to one that is not online: the UI shows it as cached.
    pub fn send(&self, id: &str, message: ClientMessage) -> bool {
        let links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        links.get(id).is_some_and(|l| {
            l.view.borrow().status == Status::Online
                && l.commands.send(Command::Send(message)).is_ok()
        })
    }

    /// Whether the machines are on screen at all (the Machines tab is open). Only then do they stream run output and
    /// terminal screens; every connection is told now and again each time it connects.
    pub fn set_all_in_view(&self, on: bool) {
        ALL_IN_VIEW.store(on, std::sync::atomic::Ordering::Relaxed);
        let links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        for link in links.values() {
            let _ = link
                .commands
                .send(Command::Send(ClientMessage::SurfaceInterest { active: on }));
        }
    }

    /// Tells the machines which one is in view: only that one streams run output.
    pub fn set_in_view(&self, id: Option<&str>) {
        let links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        for (link_id, link) in links.iter() {
            let _ = link
                .commands
                .send(Command::Send(ClientMessage::SurfaceInterest {
                    active: Some(link_id.as_str()) == id,
                }));
        }
    }
}

fn initial_view(machine: &SavedMachine) -> View {
    View {
        machine: machine.clone(),
        status: if machine.enabled {
            Status::Connecting
        } else {
            Status::Disabled
        },
        message: String::new(),
        failure: None,
        server_version: String::new(),
        threads: Vec::new(),
        runs: Vec::new(),
        activity: Vec::new(),
        panes: Vec::new(),
        frames: std::collections::HashMap::new(),
        notice: None,
    }
}

fn spawn_link(machine: SavedMachine, handle: &tokio::runtime::Handle) -> Link {
    let (view_tx, view_rx) = watch::channel(initial_view(&machine));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let task_machine = machine.clone();
    handle.spawn(async move { run_link(task_machine, view_tx, cmd_rx).await });
    Link {
        view: view_rx,
        commands: cmd_tx,
        machine,
    }
}

async fn run_link(
    machine: SavedMachine,
    view: watch::Sender<View>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let local = machine.target == ssh::LOOPBACK_TARGET;
    let mut supervisor = Supervisor::new(local, machine.enabled);
    let now = Instant::now();
    if machine.enabled {
        supervisor.enable(now);
        supervisor.retry_now(now);
    }
    loop {
        if supervisor.status() == Status::Disabled {
            // A disabled machine just waits to be stopped.
            match commands.recv().await {
                None | Some(Command::Stop) => return,
                Some(_) => continue,
            }
        }
        // Wait for the next attempt, still hearing commands.
        let wait = supervisor
            .next_attempt()
            .map_or(Duration::from_secs(3600), |at| {
                at.saturating_duration_since(Instant::now())
            });
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            command = commands.recv() => match command {
                None | Some(Command::Stop) => return,
                Some(Command::RetryNow) => { supervisor.retry_now(Instant::now()); }
                Some(Command::Send(_)) => {}
            },
        }
        if !supervisor.due(Instant::now()) {
            continue;
        }
        supervisor.attempt_started();
        publish_status(&view, &supervisor, None);
        match bridge::open(&machine, true).await {
            Err(why) => {
                let failure = ssh::classify(&why);
                supervisor.dropped(Instant::now(), failure.needs_attention());
                publish_status(&view, &supervisor, Some((why, failure)));
            }
            Ok(connection) => {
                let outcome = pump(connection, &mut supervisor, &view, &mut commands).await;
                match outcome {
                    Pumped::Stopped => return,
                    Pumped::Lost(why) => {
                        let failure = ssh::classify(&why);
                        supervisor.dropped(Instant::now(), failure.needs_attention());
                        publish_status(&view, &supervisor, Some((why, failure)));
                    }
                    Pumped::Woke => {
                        let now = Instant::now();
                        supervisor.dropped(now, false);
                        // After waking, the old backoff means nothing: try at once, then quickly for a while.
                        supervisor.power(crate::supervisor::PowerEvent::Resume, now);
                        publish_status(
                            &view,
                            &supervisor,
                            Some(("the computer woke up".to_owned(), Failure::Other)),
                        );
                    }
                }
            }
        }
    }
}

fn publish_status(
    view: &watch::Sender<View>,
    supervisor: &Supervisor,
    problem: Option<(String, Failure)>,
) {
    {
        let v = view.borrow();
        if v.status != supervisor.status() || problem.is_some() {
            vivianne_core::debuglog::log(
                "remote",
                format!(
                    "{}: {:?}{}",
                    v.machine.label,
                    supervisor.status(),
                    problem.as_ref().map_or(String::new(), |(why, _)| format!(
                        " ({})",
                        why.lines().next().unwrap_or_default()
                    ))
                ),
            );
        }
    }
    view.send_modify(|v| {
        v.status = supervisor.status();
        if let Some((why, failure)) = problem {
            v.message = why;
            v.failure = failure.needs_attention().then_some(failure);
        } else if v.status == Status::Connecting || v.status == Status::Reconnecting {
            // Keep the reason of the last failure visible while retrying.
        }
    });
}

enum Pumped {
    Stopped,
    Lost(String),
    /// The computer slept and woke: the link is dropped and retried at once (see `Supervisor::power`).
    Woke,
}

/// A gap this long between two ticks of a one-second timer means the computer was asleep, not busy.
const SLEEP_GAP: Duration = Duration::from_secs(15);

/// Runs one connection until it ends. The first snapshot makes the machine usable.
async fn pump(
    mut conn: Connection,
    supervisor: &mut Supervisor,
    view: &watch::Sender<View>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
) -> Pumped {
    let now = Instant::now();
    let mut health = Health::new(now);
    let mut next_ping = 1u64;
    let server_version = conn.negotiated.server_version.clone();
    let wants_interest = conn.negotiated.has(CAP_SURFACE_INTEREST);
    if write_frame(
        &mut conn.writer,
        &ClientMessage::SnapshotRequest { workspace: None },
    )
    .await
    .is_err()
    {
        return Pumped::Lost("the connection closed".to_owned());
    }
    let _ = wants_interest;
    // A new connection streams only if a machines page is open.
    if write_frame(
        &mut conn.writer,
        &ClientMessage::SurfaceInterest {
            active: ALL_IN_VIEW.load(std::sync::atomic::Ordering::Relaxed),
        },
    )
    .await
    .is_err()
    {
        return Pumped::Lost("the connection closed".to_owned());
    }
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_wall = std::time::SystemTime::now();
    loop {
        tokio::select! {
            frame = read_frame::<_, ServerMessage>(&mut conn.reader) => {
                let message = match frame {
                    Ok(m) => m,
                    Err(e) => return Pumped::Lost(lost_reason(&mut conn, &e.to_string()).await),
                };
                health.heard(Instant::now());
                match message {
                    ServerMessage::Snapshot { threads, runs, panes } => {
                        if !supervisor_ready(&health) {
                            vivianne_core::debuglog::log(
                                "remote",
                                format!("{}: online, server {server_version}", view.borrow().machine.label),
                            );
                            health.became_ready();
                            supervisor.connected(Instant::now());
                            view.send_modify(|v| {
                                v.status = Status::Online;
                                v.message.clear();
                                v.failure = None;
                                v.server_version.clone_from(&server_version);
                            });
                        }
                        view.send_modify(|v| {
                            v.frames.retain(|id, _| panes.iter().any(|p| &p.id == id));
                            v.threads = threads;
                            v.runs = runs;
                            v.panes = panes;
                        });
                    }
                    ServerMessage::Ping { id } => {
                        if write_frame(&mut conn.writer, &ClientMessage::Pong { id }).await.is_err() {
                            return Pumped::Lost("the connection closed".to_owned());
                        }
                    }
                    ServerMessage::Pong { .. } | ServerMessage::Unknown | ServerMessage::Welcome(_) => {}
                    ServerMessage::RunEvent { thread_id, event } => view.send_modify(|v| {
                        v.activity.push((thread_id, event));
                        if v.activity.len() > ACTIVITY_CAP { let extra = v.activity.len() - ACTIVITY_CAP; v.activity.drain(..extra); }
                    }),
                    ServerMessage::RunState { thread_id, state } => view.send_modify(|v| {
                        v.runs.retain(|r| r.thread_id != thread_id);
                        if state != AgentState::Idle { v.runs.push(RunInfo { thread_id, state }); }
                    }),
                    ServerMessage::PaneFrame { pane, lines, cursor, .. } => view.send_modify(|v| {
                        v.frames.insert(pane, (lines, cursor));
                    }),
                    ServerMessage::PaneState { pane, agent, state } => view.send_modify(|v| {
                        if let Some(p) = v.panes.iter_mut().find(|p| p.id == pane) {
                            p.agent = agent;
                            p.state = state;
                        }
                    }),
                    ServerMessage::PaneClosed { pane } => view.send_modify(|v| {
                        v.panes.retain(|p| p.id != pane);
                        v.frames.remove(&pane);
                    }),
                    ServerMessage::Notify { title, body } => view.send_modify(|v| v.notice = Some((title, body))),
                    ServerMessage::Error { code, message } => view.send_modify(|v| v.notice = Some((format!("Error ({code})"), message))),
                    ServerMessage::ServerShutdown => return Pumped::Lost("the server was stopped".to_owned()),
                }
            }
            command = commands.recv() => match command {
                None | Some(Command::Stop) => {
                    let _ = write_frame(&mut conn.writer, &ClientMessage::Detach).await;
                    return Pumped::Stopped;
                }
                Some(Command::RetryNow) => {}
                Some(Command::Send(message)) => {
                    if write_frame(&mut conn.writer, &message).await.is_err() {
                        return Pumped::Lost("the connection closed".to_owned());
                    }
                }
            },
            _ = tick.tick() => {
                let action = {
                // A gap between ticks of a one-second timer means the computer slept.
                let slept = std::mem::replace(&mut last_wall, std::time::SystemTime::now())
                    .elapsed()
                    .is_ok_and(|gap| gap >= SLEEP_GAP);
                if slept {
                    return Pumped::Woke;
                }
                health.check(Instant::now())
                };
                match action {
                health::Action::None => {}
                health::Action::Ping => {
                    health.ping_sent(Instant::now());
                    let id = next_ping; next_ping += 1;
                    if write_frame(&mut conn.writer, &ClientMessage::Ping { id }).await.is_err() {
                        return Pumped::Lost("the connection closed".to_owned());
                    }
                }
                health::Action::Expired => return Pumped::Lost("the connection stopped answering".to_owned()),
                }
            }
        }
    }
}

fn supervisor_ready(health: &Health) -> bool {
    health.is_ready()
}

/// What ssh said as it ended, if anything: the reason is usually in its stderr.
async fn lost_reason(conn: &mut Connection, fallback: &str) -> String {
    let _ = conn.child.start_kill();
    let stderr = std::mem::replace(&mut conn.stderr, tokio::spawn(async { String::new() }));
    let tail = tokio::time::timeout(Duration::from_millis(500), stderr)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if tail.trim().is_empty() {
        fallback.to_owned()
    } else {
        format!("{fallback}: {}", tail.trim())
    }
}

/// The hub's shared handle for code that has none.
#[must_use]
pub fn hub() -> &'static Hub {
    Hub::global()
}
