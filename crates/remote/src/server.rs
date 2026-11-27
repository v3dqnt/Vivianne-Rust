//! The server: one per session on a machine, started detached. It owns the saved threads and the agent runs, so
//! work goes on when the app that started it closes or the network drops, and a client that comes back finds
//! it. Clients (reached through the bridge) are told what is happening; they never own anything.
//!
//! A model key arrives from a client per connection, is held in memory for the runs that connection starts, and is
//! never written to disk, to a thread, or to a log.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Mode, Role, Thinking};
use vivianne_providers::Provider;
use vivianne_providers::ai_provider::AiProvider;
use vivianne_runtime::{Agent, AgentEvent, coding_system_prompt, store};
use vivianne_tools::Toolbox;

use crate::protocol::{
    AgentState, ClientMessage, FrameError, RunInfo, ServerMessage, ThreadInfo, WireEvent,
    read_frame, welcome_for, write_frame,
};
use crate::socket::{Listener, Stream};

/// A client has this long to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// A model key held for one connection.
#[derive(Clone)]
struct Credential {
    provider: String,
    key: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is never printed, not even by accident in a log.
        write!(f, "Credential({}, <hidden>)", self.provider)
    }
}

struct Run {
    agent: Arc<Agent>,
    cancel: CancellationToken,
    state: AgentState,
}

struct Shared {
    runs: Mutex<HashMap<String, Run>>,
    events: broadcast::Sender<ServerMessage>,
    panes: Arc<crate::panes::Panes>,
}

impl Shared {
    fn run_infos(&self) -> Vec<RunInfo> {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(id, r)| RunInfo {
                thread_id: id.clone(),
                state: r.state,
            })
            .collect()
    }

    fn set_state(&self, thread_id: &str, state: AgentState) {
        if let Some(run) = self
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(thread_id)
        {
            run.state = state;
        }
        let _ = self.events.send(ServerMessage::RunState {
            thread_id: thread_id.to_owned(),
            state,
        });
    }
}

fn snapshot(workspace: Option<&str>, shared: &Shared) -> ServerMessage {
    let threads = match workspace {
        Some(ws) => store::list(ws)
            .into_iter()
            .map(|s| ThreadInfo {
                id: s.id,
                title: s.title,
                workspace: ws.to_owned(),
                updated_at: s.updated_at,
            })
            .collect(),
        None => store::list_everywhere()
            .into_iter()
            .map(|(workspace, s)| ThreadInfo {
                id: s.id,
                title: s.title,
                workspace,
                updated_at: s.updated_at,
            })
            .collect(),
    };
    ServerMessage::Snapshot {
        threads,
        runs: shared.run_infos(),
        panes: shared.panes.infos(),
    }
}

/// Runs the server until it is stopped. Fails if this session already has one.
pub async fn run(session: &str) -> Result<(), String> {
    let mut listener = Listener::bind(session).map_err(|e| e.to_string())?;
    vivianne_core::debuglog::log(
        "remote",
        format!("server for session {session} is listening"),
    );
    let (events, _) = broadcast::channel(1024);
    let panes = crate::panes::Panes::new(session, events.clone());
    // The panes of the last run come back as fresh shells in their folders.
    panes.restore();
    let shared = Arc::new(Shared {
        runs: Mutex::new(HashMap::new()),
        events,
        panes,
    });
    let stop = CancellationToken::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok(stream) = accepted else { continue };
                let shared = shared.clone();
                let stop = stop.clone();
                tokio::spawn(async move { client(stream, shared, stop).await });
            }
            () = stop.cancelled() => {
                let _ = shared.events.send(ServerMessage::ServerShutdown);
                return Ok(());
            }
        }
    }
}

async fn client(stream: Stream, shared: Arc<Shared>, stop: CancellationToken) {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let hello = match tokio::time::timeout(
        HELLO_TIMEOUT,
        read_frame::<_, ClientMessage>(&mut reader),
    )
    .await
    {
        Ok(Ok(ClientMessage::Hello(h))) => h,
        _ => return,
    };
    let welcome = welcome_for(&hello);
    let refused = welcome.error.is_some();
    if write_frame(&mut writer, &ServerMessage::Welcome(welcome))
        .await
        .is_err()
        || refused
    {
        return;
    }

    // Everything sent to this client goes through one channel, so frames never interleave.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMessage>();
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let writer_task = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if write_frame(&mut writer, &message).await.is_err() {
                break;
            }
        }
    });
    let forward = {
        let out = out_tx.clone();
        let active = active.clone();
        let mut events = shared.events.subscribe();
        tokio::spawn(async move {
            while let Ok(message) = events.recv().await {
                // Only the machine in view streams a run's output; the others send state and notifications.
                let wanted = match &message {
                    ServerMessage::RunEvent { .. } | ServerMessage::PaneFrame { .. } => {
                        active.load(std::sync::atomic::Ordering::Relaxed)
                    }
                    _ => true,
                };
                if wanted && out.send(message).is_err() {
                    break;
                }
            }
        })
    };

    let mut credential: Option<Credential> = None;
    loop {
        let message = match read_frame::<_, ClientMessage>(&mut reader).await {
            Ok(m) => m,
            Err(FrameError::Closed | FrameError::Io(_)) => break,
            Err(e) => {
                let _ = out_tx.send(ServerMessage::Error {
                    code: "frame".to_owned(),
                    message: e.to_string(),
                });
                break;
            }
        };
        match message {
            ClientMessage::Ping { id } => {
                let _ = out_tx.send(ServerMessage::Pong { id });
            }
            ClientMessage::Pong { .. } | ClientMessage::Hello(_) | ClientMessage::Unknown => {}
            ClientMessage::SurfaceInterest { active: on } => {
                active.store(on, std::sync::atomic::Ordering::Relaxed);
            }
            ClientMessage::SessionCredential { provider, key } => {
                credential = Some(Credential { provider, key });
            }
            ClientMessage::SnapshotRequest { workspace } => {
                let _ = out_tx.send(snapshot(workspace.as_deref(), &shared));
            }
            ClientMessage::RunPrompt {
                thread_id,
                workspace,
                prompt,
                provider,
                model,
                mode,
            } => {
                start_run(
                    &shared,
                    &out_tx,
                    credential.clone(),
                    RunRequest {
                        thread_id,
                        workspace,
                        prompt,
                        provider,
                        model,
                        mode,
                    },
                );
            }
            ClientMessage::CancelRun { thread_id } => {
                if let Some(run) = shared
                    .runs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&thread_id)
                {
                    run.cancel.cancel();
                }
            }
            ClientMessage::ApprovalAnswer {
                thread_id,
                id,
                allow,
            } => {
                let agent = shared
                    .runs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&thread_id)
                    .map(|r| r.agent.clone());
                if let Some(agent) = agent
                    && agent.answer(&id, allow)
                {
                    shared.set_state(&thread_id, AgentState::Working);
                }
            }
            ClientMessage::PaneOpen {
                pane,
                workspace,
                command,
                cols,
                rows,
            } => {
                match shared
                    .panes
                    .open(&pane, &workspace, command.as_deref(), cols, rows)
                {
                    Ok(()) => {
                        // The client learns of the new pane from a fresh snapshot.
                        let _ = out_tx.send(snapshot(None, &shared));
                    }
                    Err(message) => {
                        let _ = out_tx.send(ServerMessage::Error {
                            code: "pane".to_owned(),
                            message,
                        });
                    }
                }
            }
            ClientMessage::PaneInput { pane, data } => shared.panes.input(&pane, &data),
            ClientMessage::PaneResize { pane, cols, rows } => {
                shared.panes.resize(&pane, cols, rows)
            }
            ClientMessage::PaneClose { pane } => shared.panes.close(&pane),
            ClientMessage::RegisterSshAgent { socket } => {
                if let Err(message) = shared.panes.set_ssh_agent(&socket) {
                    let _ = out_tx.send(ServerMessage::Error {
                        code: "invalid_ssh_agent".to_owned(),
                        message,
                    });
                }
            }
            ClientMessage::Detach => break,
            ClientMessage::StopServer => stop.cancel(),
        }
    }
    // The connection's key goes with it.
    drop(credential);
    forward.abort();
    drop(out_tx);
    let _ = writer_task.await;
}

struct RunRequest {
    thread_id: String,
    workspace: String,
    prompt: String,
    provider: String,
    model: String,
    mode: String,
}

fn mode_of(text: &str) -> Mode {
    match text {
        "auto" => Mode::Auto,
        "plan" => Mode::Plan,
        _ => Mode::Ask,
    }
}

/// The provider for a run: the key this connection sent for it, else the one saved on this machine.
fn provider_for(
    request: &RunRequest,
    credential: Option<&Credential>,
) -> Result<Arc<dyn Provider>, String> {
    if let Some(c) = credential.filter(|c| c.provider == request.provider) {
        return Ok(Arc::new(AiProvider::new(
            &request.provider,
            None,
            Some(c.key.clone()),
        )));
    }
    vivianne_providers::env::provider_for(&request.provider)
}

fn start_run(
    shared: &Arc<Shared>,
    out: &mpsc::UnboundedSender<ServerMessage>,
    credential: Option<Credential>,
    request: RunRequest,
) {
    if !store_id_ok(&request.thread_id) {
        let _ = out.send(ServerMessage::Error {
            code: "thread".to_owned(),
            message: "not a thread id".to_owned(),
        });
        return;
    }
    if shared
        .runs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&request.thread_id)
        .is_some_and(|r| r.state != AgentState::Idle)
    {
        let _ = out.send(ServerMessage::Error {
            code: "busy".to_owned(),
            message: "that thread is already running".to_owned(),
        });
        return;
    }
    let workdir = std::path::PathBuf::from(&request.workspace);
    if !workdir.is_dir() {
        let _ = out.send(ServerMessage::Error {
            code: "workspace".to_owned(),
            message: format!("{} is not a folder on this machine", request.workspace),
        });
        return;
    }
    let provider = match provider_for(&request, credential.as_ref()) {
        Ok(p) => p,
        Err(message) => {
            let _ = out.send(ServerMessage::Error {
                code: "provider".to_owned(),
                message,
            });
            return;
        }
    };
    let settings = vivianne_runtime::config::resolve(&workdir, None);
    let mut toolbox = Toolbox::coding(&workdir);
    if let Some(warning) = vivianne_runtime::surface::prepare(&mut toolbox, &workdir, &settings) {
        eprintln!("viv: {warning}");
    }
    let mut built = Agent::new(
        provider,
        &request.model,
        Some(coding_system_prompt(&workdir)),
        toolbox,
    )
    .with_max_steps(settings.max_agent_steps.value)
    // The project's instruction files, the clock and the terminal's place, as the app adds them.
    .with_instructions(vec![workdir.display().to_string()]);
    if let Ok(recorder) =
        vivianne_runtime::changes::Recorder::new(&workdir, settings.change_limits())
    {
        built = built.with_changes(Arc::new(recorder));
    }
    let agent = Arc::new(built);
    agent.set_mode(mode_of(&request.mode));
    let cancel = CancellationToken::new();
    shared
        .runs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            request.thread_id.clone(),
            Run {
                agent: agent.clone(),
                cancel: cancel.clone(),
                state: AgentState::Working,
            },
        );
    let _ = shared.events.send(ServerMessage::RunState {
        thread_id: request.thread_id.clone(),
        state: AgentState::Working,
    });
    let shared = shared.clone();
    tokio::spawn(async move {
        let thread_id = request.thread_id.clone();
        // One writer per conversation: a run is refused while another Vivianne is writing this one.
        let _lease = match store::lease(&thread_id) {
            Ok(lease) => lease,
            Err(why) => {
                let _ = shared.events.send(ServerMessage::RunEvent {
                    thread_id: thread_id.clone(),
                    event: WireEvent::Failed { message: why.to_string() },
                });
                shared.set_state(&thread_id, AgentState::Idle);
                shared
                    .runs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&thread_id);
                return;
            }
        };
        // Saved before every model request and before a step's tools run, so a crash loses at most the step in flight.
        {
            let (id, workspace, provider, model, mode) = (
                thread_id.clone(),
                request.workspace.clone(),
                request.provider.clone(),
                request.model.clone(),
                mode_of(&request.mode),
            );
            agent.set_checkpoint(Some(vivianne_runtime::Checkpoint::new(move |history| {
                store::checkpoint(&id, &workspace, &provider, &model, mode, history)
            })));
        }
        let mut history = store::load(&thread_id)
            .map(|t| t.history)
            .unwrap_or_default();
        let first = history.is_empty();
        history.push(Message::new(Role::User, request.prompt.clone()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let relay = {
            let shared = shared.clone();
            let thread_id = thread_id.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    relay_event(&shared, &thread_id, &event);
                }
            })
        };
        let result = agent
            .run_until(&mut history, &tx, Thinking::default(), &cancel)
            .await;
        drop(tx);
        let _ = relay.await;
        vivianne_core::debuglog::log(
            "run",
            format!(
                "remote run {thread_id} ended: {}",
                if result.is_ok() { "ok" } else { "failed" }
            ),
        );
        let wire = match result {
            Ok(()) if cancel.is_cancelled() => WireEvent::Stopped,
            Ok(()) => WireEvent::Done,
            Err(e) => WireEvent::Failed {
                message: e.to_string(),
            },
        };
        // The conversation is kept either way, so a client that comes back finds it.
        let title = if first {
            vivianne_runtime::title::fallback_title(&request.prompt)
        } else {
            store::load(&thread_id).map(|t| t.title).unwrap_or_default()
        };
        let _ = store::save(&store::SavedThread {
            id: thread_id.clone(),
            workspace: request.workspace.clone(),
            title,
            title_source: "auto".to_owned(),
            provider: request.provider.clone(),
            model: request.model.clone(),
            mode: mode_of(&request.mode),
            updated_at: store::now(),
            history,
            parent: None,
        });
        let _ = shared.events.send(ServerMessage::RunEvent {
            thread_id: thread_id.clone(),
            event: wire.clone(),
        });
        let _ = shared.events.send(ServerMessage::Notify {
            title: "Run finished".to_owned(),
            body: match wire {
                WireEvent::Failed { message } => message,
                _ => thread_id.clone(),
            },
        });
        shared.set_state(&thread_id, AgentState::Idle);
        shared
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&thread_id);
        // Idle runs are not listed: they are just threads.
        let _ = shared.events.send(ServerMessage::RunState {
            thread_id,
            state: AgentState::Idle,
        });
    });
}

fn store_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn relay_event(shared: &Shared, thread_id: &str, event: &AgentEvent) {
    let wire = match event {
        AgentEvent::Text(text) => Some(WireEvent::Text { text: text.clone() }),
        AgentEvent::ToolStart { name, args, .. } => Some(WireEvent::ToolStart {
            name: name.clone(),
            summary: summary(args),
        }),
        AgentEvent::ToolEnd { name, is_error, .. } => Some(WireEvent::ToolEnd {
            name: name.clone(),
            is_error: *is_error,
        }),
        AgentEvent::Approval { id, title } => {
            shared.set_state(thread_id, AgentState::Blocked);
            Some(WireEvent::Approval {
                id: id.clone(),
                title: title.clone(),
            })
        }
        _ => None,
    };
    if let Some(event) = wire {
        let _ = shared.events.send(ServerMessage::RunEvent {
            thread_id: thread_id.to_owned(),
            event,
        });
    }
}

/// A tool call in a line: its command or path, cut short.
fn summary(args: &serde_json::Value) -> String {
    let text = ["command", "path", "pattern", "query"]
        .iter()
        .find_map(|k| args[k].as_str())
        .unwrap_or_default();
    text.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_never_prints_its_key() {
        let c = Credential {
            provider: "openai".into(),
            key: "sk-secret-secret-secret".into(),
        };
        let shown = format!("{c:?}");
        assert!(!shown.contains("secret") && shown.contains("openai"));
    }

    /// A real server on a real socket: handshake, ping, snapshot, a refused generation, and stopping it.
    #[tokio::test]
    async fn a_client_can_shake_hands_ping_ask_for_a_snapshot_and_stop_the_server() {
        use crate::protocol::{ClientHello, client_hello};
        let session = format!("t{}", uuid::Uuid::new_v4().simple());
        let server = {
            let session = session.clone();
            tokio::spawn(async move { run(&session).await })
        };
        // The server binds in the background; wait until it answers.
        for _ in 0..50 {
            if crate::socket::is_listening(&session).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut c = crate::socket::connect(&session).await.expect("connects");
        write_frame(
            &mut c,
            &ClientMessage::Hello(client_hello(crate::CAP_WANTED)),
        )
        .await
        .expect("hello");
        let ServerMessage::Welcome(w) = read_frame(&mut c).await.expect("welcome") else {
            panic!("not a welcome")
        };
        assert!(crate::protocol::accept_welcome(w).is_ok());
        write_frame(&mut c, &ClientMessage::Ping { id: 9 })
            .await
            .expect("ping");
        assert_eq!(
            read_frame::<_, ServerMessage>(&mut c).await.expect("pong"),
            ServerMessage::Pong { id: 9 }
        );
        write_frame(
            &mut c,
            &ClientMessage::SnapshotRequest {
                workspace: Some("no-such-project".into()),
            },
        )
        .await
        .expect("snapshot");
        let ServerMessage::Snapshot { threads, runs, .. } =
            read_frame(&mut c).await.expect("snapshot")
        else {
            panic!("not a snapshot")
        };
        assert!(threads.is_empty() && runs.is_empty());

        // A client of another generation is told why and cut off.
        let mut old = crate::socket::connect(&session).await.expect("connects");
        let mut hello: ClientHello = client_hello(&[]);
        hello.generation = 0;
        write_frame(&mut old, &ClientMessage::Hello(hello))
            .await
            .expect("hello");
        let ServerMessage::Welcome(w) = read_frame(&mut old).await.expect("welcome") else {
            panic!("not a welcome")
        };
        assert_eq!(w.error.expect("refused").code, "generation");

        write_frame(&mut c, &ClientMessage::StopServer)
            .await
            .expect("stop");
        let ended = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("stops in time")
            .expect("joins");
        assert!(ended.is_ok());
    }

    #[test]
    fn modes_default_to_asking_and_thread_ids_cannot_name_a_path() {
        assert_eq!(mode_of("auto"), Mode::Auto);
        assert_eq!(mode_of("plan"), Mode::Plan);
        assert_eq!(mode_of("anything else"), Mode::Ask);
        assert!(store_id_ok("3f2a-91") && !store_id_ok("../x") && !store_id_ok(""));
    }
}
