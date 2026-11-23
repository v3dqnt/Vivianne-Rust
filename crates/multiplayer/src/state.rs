//! The connection: one endpoint per app, always listening (so either side can be the one dialled), one partner at a
//! time. It holds the chat, hands out events for the UI, and runs the collaboration around each agent run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointId};
use tokio::sync::{RwLock, broadcast, mpsc};
use vivianne_core::{Message, Role};

use crate::collab::{self, ActivityEntry, MAX_BUNDLE_BYTES, SyncNote, now_ms};
use crate::protocol::{self, ALPN, DayUsage, PROTOCOL_VERSION, WireMessage};

/// Chat and mirrored entries kept in memory. A live socket feeding an unbounded list is how a long pairing session
/// turns into a memory leak.
const MAX_CHAT_ENTRIES: usize = 500;
/// A partner's display name is shown as it comes ("Paired with ..."), so it is capped.
const MAX_DISPLAY_NAME_CHARS: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
/// A write that takes longer than this means the partner has stopped reading: drop the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(20);
const OUTBOUND_QUEUE: usize = 256;

/// A project that is paired with a partner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub id: String,
    pub folder: PathBuf,
    pub peer_id: String,
    pub peer_display_name: Option<String>,
    /// A git remote both sides use; without one, commits go over the connection.
    pub git_remote: Option<String>,
}

/// How the app says which projects are paired, and with whom. A connection from anyone who is not the partner of
/// some paired project is closed before a byte is exchanged: knowing a pairing code is not being let in.
pub trait Projects: Send + Sync {
    fn for_peer(&self, peer_id: &str) -> Option<Paired>;
    fn for_folder(&self, folder: &Path) -> Option<Paired>;
    /// What this side spent on the project, by day, to tell the partner.
    fn usage(&self, _project: &Paired) -> std::collections::BTreeMap<String, DayUsage> {
        std::collections::BTreeMap::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    Connecting,
    Connected {
        peer_id: String,
        peer_display_name: Option<String>,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatEntry {
    pub from_peer: bool,
    pub text: String,
    pub at: u64,
}

/// What the UI hears about.
#[derive(Debug, Clone)]
pub enum Event {
    Status(Status),
    Chat(ChatEntry),
    /// Something changed in a project's shared record: its id.
    Collab(String),
}

pub struct Multiplayer {
    /// The pairing code to give a partner.
    pub my_id: String,
    endpoint: Endpoint,
    projects: Arc<dyn Projects>,
    status: RwLock<Status>,
    display_name: RwLock<Option<String>>,
    /// The writer's queue, while a connection is alive. Sending hands a frame to it and returns: only the writer
    /// touches the network, so a stalled partner stalls one task and nothing else.
    outbound: RwLock<Option<mpsc::Sender<WireMessage>>>,
    chat: RwLock<Vec<ChatEntry>>,
    /// Bumped on each new connection, so a loop from one that was replaced stops touching shared state.
    generation: AtomicU64,
    /// Runs in progress, so a partner's commits merge at once only when no agent is working.
    runs: AtomicUsize,
    events: broadcast::Sender<Event>,
}

fn default_display_name() -> Option<String> {
    // VIV_DISPLAY_NAME names a second instance on one machine (a test partner), which would otherwise introduce
    // itself with the same user name as the first.
    std::env::var("VIV_DISPLAY_NAME")
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("USER"))
        .ok()
}

fn cap_name(name: Option<String>) -> Option<String> {
    name.map(|n| n.chars().take(MAX_DISPLAY_NAME_CHARS).collect::<String>())
        .filter(|n| !n.is_empty())
}

fn push_bounded<T>(list: &mut Vec<T>, item: T) {
    list.push(item);
    if list.len() > MAX_CHAT_ENTRIES {
        let excess = list.len() - MAX_CHAT_ENTRIES;
        list.drain(..excess);
    }
}

/// A run is in progress; see [`Multiplayer::before_run`].
#[derive(Debug)]
pub struct RunGuard(Arc<Multiplayer>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.runs.fetch_sub(1, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for Multiplayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Multiplayer")
            .field("my_id", &self.my_id)
            .finish_non_exhaustive()
    }
}

static GLOBAL: Mutex<Option<Arc<Multiplayer>>> = Mutex::new(None);

/// The running multiplayer, once it has started.
#[must_use]
pub fn global() -> Option<Arc<Multiplayer>> {
    GLOBAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

impl Multiplayer {
    /// Opens the endpoint and starts accepting partners. Call once, on the async runtime.
    ///
    /// # Errors
    /// The identity could not be kept, or the endpoint could not open (no network).
    pub async fn start(projects: Arc<dyn Projects>) -> Result<Arc<Self>, String> {
        let key = crate::identity::load_or_create()?;
        let my_id = key.public().to_string();
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(key)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| format!("couldn't start multiplayer: {e}"))?;
        let (events, _) = broadcast::channel(64);
        let this = Arc::new(Self {
            my_id,
            endpoint,
            projects,
            status: RwLock::new(Status::Disconnected),
            display_name: RwLock::new(default_display_name()),
            outbound: RwLock::new(None),
            chat: RwLock::new(Vec::new()),
            generation: AtomicU64::new(0),
            runs: AtomicUsize::new(0),
            events,
        });
        *GLOBAL.lock().unwrap_or_else(PoisonError::into_inner) = Some(this.clone());
        tokio::spawn(this.clone().accept_loop());
        Ok(this)
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    fn emit(&self, event: Event) {
        // No one listening is fine.
        let _ = self.events.send(event);
    }

    async fn set_status(&self, status: Status) {
        *self.status.write().await = status.clone();
        self.emit(Event::Status(status));
    }

    pub async fn status(&self) -> Status {
        self.status.read().await.clone()
    }

    pub async fn display_name(&self) -> Option<String> {
        self.display_name.read().await.clone()
    }

    pub async fn set_display_name(&self, name: Option<String>) {
        *self.display_name.write().await = cap_name(name);
    }

    pub async fn chat_log(&self) -> Vec<ChatEntry> {
        self.chat.read().await.clone()
    }

    /// Listens for the rest of the app's life: either side can be the one dialled.
    async fn accept_loop(self: Arc<Self>) {
        while let Some(incoming) = self.endpoint.accept().await {
            let this = self.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                let peer = conn.remote_id().to_string();
                if this.projects.for_peer(&peer).is_none() {
                    conn.close(1_u32.into(), b"not a partner of any paired project");
                    return;
                }
                this.adopt(conn, false).await;
            });
        }
    }

    /// Dials a partner by their pairing code.
    ///
    /// # Errors
    /// The code is not one, or the partner could not be reached.
    pub async fn connect(self: &Arc<Self>, code: &str) -> Result<(), String> {
        let peer: EndpointId = code
            .trim()
            .parse()
            .map_err(|_| "That doesn't look like a valid pairing code.".to_owned())?;
        self.set_status(Status::Connecting).await;
        // Our own side has to be reachable too, or the partner cannot answer.
        let _ = tokio::time::timeout(Duration::from_secs(10), self.endpoint.online()).await;
        let conn = match tokio::time::timeout(CONNECT_TIMEOUT, self.endpoint.connect(peer, ALPN))
            .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(self.fail(format!("Couldn't reach that partner: {e}")).await),
            Err(_) => {
                return Err(self
                    .fail("Timed out trying to reach that partner.".to_owned())
                    .await);
            }
        };
        tokio::spawn(self.clone().adopt(conn, true));
        Ok(())
    }

    async fn fail(&self, reason: String) -> String {
        self.set_status(Status::Failed {
            reason: reason.clone(),
        })
        .await;
        reason
    }

    /// Both sides, once a connection exists: the dialler opens the one stream, the accepter waits for it, both say
    /// hello, then reading and a dedicated writer start.
    async fn adopt(self: Arc<Self>, conn: Connection, dialler: bool) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let peer = conn.remote_id().to_string();
        let streams = if dialler {
            conn.open_bi().await
        } else {
            conn.accept_bi().await
        };
        let Ok((mut send, mut recv)) = streams else {
            self.lost(generation).await;
            return;
        };
        let hello = WireMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            display_name: self.display_name().await,
        };
        if protocol::write_frame(&mut send, &hello).await.is_err() {
            return;
        }
        let partner_name = match protocol::read_frame(&mut recv).await {
            Ok(Some(WireMessage::Hello {
                protocol_version,
                display_name,
            })) => {
                if protocol_version != PROTOCOL_VERSION {
                    if self.current(generation) {
                        self.set_status(Status::Failed {
                            reason: "Your partner is running an incompatible version of Vivianne."
                                .to_owned(),
                        })
                        .await;
                    }
                    return;
                }
                cap_name(display_name).map(|n| collab::peer_text(&n, MAX_DISPLAY_NAME_CHARS))
            }
            _ => None,
        };
        // A newer connection took over while this one was shaking hands.
        if !self.current(generation) {
            return;
        }
        let (tx, rx) = mpsc::channel(OUTBOUND_QUEUE);
        *self.outbound.write().await = Some(tx);
        tokio::spawn(self.clone().write_loop(send, rx, generation));
        self.set_status(Status::Connected {
            peer_id: peer.clone(),
            peer_display_name: partner_name,
        })
        .await;
        tokio::spawn(self.clone().on_connected(peer.clone()));
        self.read_loop(recv, generation, peer).await;
    }

    fn current(&self, generation: u64) -> bool {
        generation == self.generation.load(Ordering::SeqCst)
    }

    /// A connection ended: if it is still the current one, there is no connection.
    async fn lost(&self, generation: u64) {
        if self.current(generation) {
            *self.outbound.write().await = None;
            self.set_status(Status::Disconnected).await;
        }
    }

    async fn write_loop(
        self: Arc<Self>,
        mut send: SendStream,
        mut queue: mpsc::Receiver<WireMessage>,
        generation: u64,
    ) {
        while let Some(msg) = queue.recv().await {
            match tokio::time::timeout(WRITE_TIMEOUT, protocol::write_frame(&mut send, &msg)).await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => break,
            }
        }
        self.lost(generation).await;
    }

    async fn read_loop(self: Arc<Self>, mut recv: RecvStream, generation: u64, peer: String) {
        loop {
            match protocol::read_frame(&mut recv).await {
                Ok(Some(WireMessage::Chat { text })) => {
                    let entry = ChatEntry {
                        from_peer: true,
                        text: collab::peer_text(&text, 4_000),
                        at: now_ms(),
                    };
                    push_bounded(&mut *self.chat.write().await, entry.clone());
                    self.emit(Event::Chat(entry));
                }
                // Only expected once, at the handshake; and the old app's mirrored messages are not used here.
                Ok(Some(WireMessage::Hello { .. } | WireMessage::SessionMessage { .. })) => {}
                Ok(Some(msg)) => {
                    // Off the read loop: merging commits runs git, which must not hold up the next frame.
                    tokio::spawn(self.clone().on_message(peer.clone(), msg));
                }
                Ok(None) | Err(_) => break,
            }
        }
        self.lost(generation).await;
    }

    pub async fn disconnect(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.outbound.write().await = None;
        self.set_status(Status::Disconnected).await;
    }

    /// Queues a frame for the partner. False when there is no connection or the queue is full.
    pub async fn send(&self, msg: WireMessage) -> bool {
        let Some(tx) = self.outbound.read().await.clone() else {
            return false;
        };
        tx.try_send(msg).is_ok()
    }

    /// Says something to the partner.
    ///
    /// # Errors
    /// Not connected, or the connection is busy.
    pub async fn send_chat(&self, text: String) -> Result<(), String> {
        let tx = self
            .outbound
            .read()
            .await
            .clone()
            .ok_or("Not connected to a partner.")?;
        tx.try_send(WireMessage::Chat { text: text.clone() })
            .map_err(|_| "The connection is busy, try again.".to_owned())?;
        let entry = ChatEntry {
            from_peer: false,
            text,
            at: now_ms(),
        };
        push_bounded(&mut *self.chat.write().await, entry.clone());
        self.emit(Event::Chat(entry));
        Ok(())
    }

    // ── Collaboration ─────────────────────────────────────────────────────────────────────────────────────────

    async fn my_name(&self) -> String {
        self.display_name()
            .await
            .unwrap_or_else(|| "Partner".to_owned())
    }

    /// A connection to `peer` opened: tell them our HEAD, give them a moment to tell us theirs, then send what they
    /// are owed.
    async fn on_connected(self: Arc<Self>, peer: String) {
        let Some(project) = self.projects.for_peer(&peer) else {
            return;
        };
        if let Some(head) = collab::head(&project.folder).await {
            self.send(WireMessage::GitHead { head }).await;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        self.flush(&project).await;
    }

    /// Sends the partner everything they are owed: undelivered activity, the commits they lack (when there is no
    /// shared remote), and our usage.
    async fn flush(&self, project: &Paired) {
        let (unsent, peer_head) = collab::with_store(&project.id, |s| {
            let ids: std::collections::HashSet<String> = s.unsent.drain(..).collect();
            (
                s.my_activity
                    .iter()
                    .filter(|e| ids.contains(&e.id))
                    .cloned()
                    .collect::<Vec<ActivityEntry>>(),
                s.peer_head.clone(),
            )
        });
        for entry in unsent {
            self.send(WireMessage::Activity {
                entry: Box::new(entry),
            })
            .await;
        }
        if project
            .git_remote
            .as_deref()
            .is_none_or(|r| r.trim().is_empty())
        {
            match collab::bundle_since(&project.folder, peer_head.as_deref()).await {
                Ok(Some(b)) if b.bytes.len() <= MAX_BUNDLE_BYTES => {
                    let data = base64::engine::general_purpose::STANDARD.encode(&b.bytes);
                    self.send(WireMessage::GitBundle {
                        base: b.base,
                        head: b.head,
                        data,
                    })
                    .await;
                }
                Ok(Some(b)) => {
                    let text = format!(
                        "New commits are too large to send directly ({} MB): add a shared git remote to this project.",
                        b.bytes.len() / 1_000_000
                    );
                    collab::with_store(&project.id, |s| {
                        s.notes.push(SyncNote {
                            at: now_ms(),
                            text,
                            shown_to_agent: false,
                        })
                    });
                }
                Ok(None) | Err(_) => {}
            }
        }
        self.send(WireMessage::Usage {
            days: self.projects.usage(project),
        })
        .await;
    }

    /// A collaboration frame from `peer`. Everything in it is untrusted.
    async fn on_message(self: Arc<Self>, peer: String, msg: WireMessage) {
        let Some(project) = self.projects.for_peer(&peer) else {
            return;
        };
        match msg {
            WireMessage::Activity { entry } => {
                if collab::record_peer_entry(&project.id, *entry) {
                    self.emit(Event::Collab(project.id.clone()));
                }
            }
            WireMessage::Usage { days } => {
                let days = collab::sanitize_usage(days);
                collab::with_store(&project.id, |s| s.peer_usage = days);
                self.emit(Event::Collab(project.id.clone()));
            }
            WireMessage::GitHead { head } => {
                let head: String = head
                    .chars()
                    .filter(char::is_ascii_hexdigit)
                    .take(64)
                    .collect();
                collab::with_store(&project.id, |s| s.peer_head = Some(head));
            }
            WireMessage::GitBundle { data, .. } => self.take_bundle(&project, &data).await,
            WireMessage::Hello { .. }
            | WireMessage::SessionMessage { .. }
            | WireMessage::Chat { .. } => {}
        }
    }

    /// The partner's commits arrived: fetch them, and merge now if no agent is working, else before the next run.
    async fn take_bundle(&self, project: &Paired, data: &str) {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return;
        };
        if bytes.len() > MAX_BUNDLE_BYTES {
            return;
        }
        if let Err(e) = collab::fetch_bundle(&project.folder, &bytes).await {
            let text = format!(
                "Couldn't take your partner's commits: {}",
                collab::first_line_of(&e, 200)
            );
            collab::with_store(&project.id, |s| {
                s.notes.push(SyncNote {
                    at: now_ms(),
                    text,
                    shown_to_agent: false,
                })
            });
            self.emit(Event::Collab(project.id.clone()));
            return;
        }
        collab::with_store(&project.id, |s| s.pending_merge = true);
        if self.runs.load(Ordering::SeqCst) == 0 {
            self.merge_pending(project).await;
        }
        self.emit(Event::Collab(project.id.clone()));
    }

    async fn merge_pending(&self, project: &Paired) {
        if !collab::read(&project.id).pending_merge {
            return;
        }
        let author = self.my_name().await;
        let result = collab::merge_partner(&project.folder, &author).await;
        let head = collab::head(&project.folder).await;
        collab::with_store(&project.id, |s| {
            s.pending_merge = false;
            match &result {
                Ok(()) => {
                    s.last_sync = Some(format!(
                        "Merged your partner's changes at {}",
                        chrono::Local::now().format("%H:%M")
                    ))
                }
                Err(note) => s.notes.push(SyncNote {
                    at: now_ms(),
                    text: note.clone(),
                    shown_to_agent: false,
                }),
            }
        });
        if let (Ok(()), Some(head)) = (&result, head) {
            self.send(WireMessage::GitHead { head }).await;
        }
        self.emit(Event::Collab(project.id.clone()));
    }

    /// Right before a run in `folder`: brings in the partner's pending commits, then adds to `history` (before the
    /// message the run is about) what the partner did that the agent has not seen. Nothing for an unpaired project.
    ///
    /// Hold the returned guard for the run: while one is held, a partner's commits wait for the run to end instead
    /// of being merged under the agent's feet. Dropping it (the run ended, or was stopped) lets them in.
    pub async fn before_run(
        self: &Arc<Self>,
        folder: &Path,
        history: &mut Vec<Message>,
    ) -> RunGuard {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let guard = RunGuard(self.clone());
        let Some(project) = self.projects.for_folder(folder) else {
            return guard;
        };
        self.merge_pending(&project).await;
        let partner = project.peer_display_name.as_deref().map_or_else(
            || "your partner".to_owned(),
            |n| collab::peer_text(n, 60).replace('"', "'"),
        );
        let block = collab::with_store(&project.id, |s| {
            let fresh: Vec<ActivityEntry> = s
                .peer_activity
                .iter()
                .filter(|e| e.at > s.seen_upto)
                .cloned()
                .collect();
            let notes: Vec<String> = s
                .notes
                .iter()
                .filter(|n| !n.shown_to_agent)
                .map(|n| n.text.clone())
                .collect();
            let block = collab::activity_block(&partner, &fresh, &s.my_activity, &notes);
            if block.is_some() {
                s.seen_upto = s
                    .peer_activity
                    .iter()
                    .map(|e| e.at)
                    .max()
                    .unwrap_or(s.seen_upto);
                s.notes.iter_mut().for_each(|n| n.shown_to_agent = true);
            }
            block
        });
        if let Some(text) = block {
            // Before the message this run is about, which is the last.
            let at = history.len().saturating_sub(1);
            history.insert(at, Message::new(Role::User, text));
        }
        guard
    }

    /// After a run in `folder`: records and sends what it did, commits and syncs the code, sends usage.
    pub async fn after_run(&self, folder: &Path, request: &str, new_messages: &[Message]) {
        let Some(project) = self.projects.for_folder(folder) else {
            return;
        };
        let author = self.my_name().await;
        let mut entry = collab::summarise_run(
            &author,
            request,
            new_messages,
            &folder.display().to_string(),
        );
        if let Ok(sha) =
            collab::commit_all(folder, &author, &format!("{author}: {}", entry.request)).await
        {
            entry.commit = sha;
        }
        // Nothing touched and nothing said: not worth telling the partner.
        if !entry.files.is_empty() || !entry.commands.is_empty() || !entry.outcome.is_empty() {
            let stored = entry.clone();
            collab::with_store(&project.id, |s| {
                s.unsent.push(stored.id.clone());
                s.my_activity.push(stored);
                collab::bounded(&mut s.my_activity);
            });
        }
        if project
            .git_remote
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty())
            && entry.commit.is_some()
        {
            let result = collab::sync_remote(folder).await;
            collab::with_store(&project.id, |s| match &result {
                Ok(()) => {
                    s.last_sync = Some(format!(
                        "Pushed to the shared remote at {}",
                        chrono::Local::now().format("%H:%M")
                    ))
                }
                Err(e) => s.notes.push(SyncNote {
                    at: now_ms(),
                    text: format!(
                        "Sync with the shared remote failed: {}",
                        collab::first_line_of(e, 200)
                    ),
                    shown_to_agent: false,
                }),
            });
        }
        if matches!(self.status().await, Status::Connected { ref peer_id, .. } if *peer_id == project.peer_id)
        {
            self.flush(&project).await;
        }
        self.emit(Event::Collab(project.id));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Two endpoints on this machine reach each other by code and trade the handshake: the path two windows take.
    /// Needs the network (discovery goes through the n0 relay): `cargo test -p vivianne-multiplayer -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the network"]
    async fn two_endpoints_pair() {
        let make = || async {
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(iroh::SecretKey::generate())
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
                .expect("binds")
        };
        let (a, b) = (make().await, make().await);
        // A new endpoint is found by others only once it has reached its relay and published where it is.
        tokio::join!(a.online(), b.online());
        tokio::time::sleep(Duration::from_secs(3)).await;
        let b_id = b.id();
        let server = tokio::spawn(async move {
            let conn = b.accept().await.expect("incoming").await.expect("accepted");
            let (mut send, mut recv) = conn.accept_bi().await.expect("stream");
            let hello = protocol::read_frame(&mut recv).await;
            protocol::write_frame(
                &mut send,
                &WireMessage::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    display_name: Some("B".into()),
                },
            )
            .await
            .expect("writes");
            send.finish().expect("finishes");
            let _ = conn.closed().await;
            hello
        });
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, a.connect(b_id, ALPN))
            .await
            .expect("no timeout")
            .expect("connects");
        let (mut send, mut recv) = conn.open_bi().await.expect("stream");
        protocol::write_frame(
            &mut send,
            &WireMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                display_name: Some("A".into()),
            },
        )
        .await
        .expect("writes");
        let reply = protocol::read_frame(&mut recv).await;
        conn.close(0_u32.into(), b"done");
        assert!(matches!(
            server.await.expect("joins"),
            Ok(Some(WireMessage::Hello { .. }))
        ));
        assert!(matches!(reply, Ok(Some(WireMessage::Hello { .. }))));
    }
}
