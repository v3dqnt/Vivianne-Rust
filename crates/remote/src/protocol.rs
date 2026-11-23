//! The messages between the app (the client) and a machine's server, and how they are framed.
//!
//! A frame is a 4-byte little-endian length and then that many bytes of JSON. Frames are read and written the same
//! way in both directions, over a local socket and, through the bridge, over SSH. Compatibility is by
//! *capabilities*, not by build: the handshake compares an endpoint generation and what each side can do, so a
//! newer client works with an older server for what the older server can do.
//!
//! New fields must be optional or defaulted, and a message this build does not know reads as `Unknown` and is
//! ignored, so an older side never fails on a newer one's extra message.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// Bumped when the framing or a message changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;
/// Bumped when a server and client can no longer talk at all. Both sides must have the same one.
pub const ENDPOINT_GENERATION: u32 = 1;
/// The most a frame may hold. A bigger claim is a broken or hostile peer.
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

pub const CAP_SURFACE_INTEREST: &str = "surface_interest";
pub const CAP_HEALTH_CHECK: &str = "health_check";
pub const CAP_SESSION_CREDENTIAL: &str = "session_credential";
pub const CAP_AGENT_RUNS: &str = "agent_runs";
pub const CAP_AGENT_STATE: &str = "agent_state";
pub const CAP_SSH_AGENT_REGISTRATION: &str = "ssh_agent_registration";
pub const CAP_TERMINAL_PANES: &str = "terminal_panes";

/// What this build's server can do.
#[must_use]
pub fn server_capabilities() -> Vec<String> {
    [
        CAP_SURFACE_INTEREST,
        CAP_HEALTH_CHECK,
        CAP_SESSION_CREDENTIAL,
        CAP_AGENT_RUNS,
        CAP_AGENT_STATE,
        CAP_SSH_AGENT_REGISTRATION,
        CAP_TERMINAL_PANES,
    ]
    .iter()
    .map(|c| (*c).to_owned())
    .collect()
}

/// A saved conversation as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub updated_at: u64,
}

/// A run the server is doing (or has done since it started).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInfo {
    pub thread_id: String,
    pub state: AgentState,
}

/// A terminal pane on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: String,
    pub workspace: String,
    #[serde(default)]
    pub command: Option<String>,
    /// The coding agent running in it, when one is recognised.
    #[serde(default)]
    pub agent: Option<String>,
    pub state: AgentState,
    #[serde(default)]
    pub title: String,
}

/// What an agent is doing. Native runs report it exactly from their events; terminal panes are read off the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Idle,
    Working,
    /// Waiting for the person: an approval, a question.
    Blocked,
    Unknown,
}

/// What a run did, as the client needs to show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireEvent {
    Text {
        text: String,
    },
    ToolStart {
        name: String,
        summary: String,
    },
    ToolEnd {
        name: String,
        is_error: bool,
    },
    Approval {
        id: String,
        title: String,
    },
    Done,
    Stopped,
    Failed {
        message: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientHello {
    pub generation: u32,
    pub protocol_version: u32,
    pub client_version: String,
    /// What the client would like to use.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerWelcome {
    pub generation: u32,
    pub protocol_version: u32,
    pub server_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Set when the server will not talk to this client (and says why).
    #[serde(default)]
    pub error: Option<HandshakeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeError {
    pub code: String,
    pub message: String,
}

/// From the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello(ClientHello),
    Ping {
        id: u64,
    },
    Pong {
        id: u64,
    },
    /// Which machine is in view: only that one streams run output. Others send state and notifications.
    SurfaceInterest {
        active: bool,
    },
    /// A model key for this connection's runs. Held in memory by the server, never written anywhere.
    SessionCredential {
        provider: String,
        key: String,
    },
    SnapshotRequest {
        #[serde(default)]
        workspace: Option<String>,
    },
    RunPrompt {
        thread_id: String,
        workspace: String,
        prompt: String,
        provider: String,
        model: String,
        /// `ask`, `auto` or `plan`.
        mode: String,
    },
    CancelRun {
        thread_id: String,
    },
    ApprovalAnswer {
        thread_id: String,
        id: String,
        allow: bool,
    },
    /// Opens a terminal pane on the server, in `workspace` (the server's folder when empty), running `command` first.
    PaneOpen {
        pane: String,
        workspace: String,
        #[serde(default)]
        command: Option<String>,
        cols: u16,
        rows: u16,
    },
    /// Keys typed into a pane.
    PaneInput {
        pane: String,
        data: String,
    },
    PaneResize {
        pane: String,
        cols: u16,
        rows: u16,
    },
    PaneClose {
        pane: String,
    },
    /// The client's SSH agent socket on the server's machine (a forwarded one), so git and signing work in panes.
    /// Only sent when the person has set up forwarding for that host; this app never turns it on.
    RegisterSshAgent {
        socket: String,
    },
    Detach,
    /// Stop this session's server (`viv server stop`). Runs in progress end.
    StopServer,
    #[serde(other)]
    Unknown,
}

/// From the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome(ServerWelcome),
    Ping {
        id: u64,
    },
    Pong {
        id: u64,
    },
    Snapshot {
        threads: Vec<ThreadInfo>,
        runs: Vec<RunInfo>,
        #[serde(default)]
        panes: Vec<PaneInfo>,
    },
    /// The screen of a pane in view: its lines as text and where the cursor is.
    PaneFrame {
        pane: String,
        lines: Vec<String>,
        cursor: (u16, u16),
        #[serde(default)]
        title: String,
    },
    PaneState {
        pane: String,
        agent: Option<String>,
        state: AgentState,
    },
    PaneClosed {
        pane: String,
    },
    RunEvent {
        thread_id: String,
        event: WireEvent,
    },
    RunState {
        thread_id: String,
        state: AgentState,
    },
    Notify {
        title: String,
        body: String,
    },
    Error {
        code: String,
        message: String,
    },
    ServerShutdown,
    #[serde(other)]
    Unknown,
}

/// Why a frame could not be read.
#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    /// The peer claimed a frame bigger than the limit.
    Oversized {
        claimed: usize,
        max: usize,
    },
    Json(String),
    /// The stream ended (cleanly between frames, or in the middle of one).
    Closed,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Oversized { claimed, max } => {
                write!(f, "a frame of {claimed} bytes is over the {max} limit")
            }
            Self::Json(e) => write!(f, "a frame could not be read: {e}"),
            Self::Closed => write!(f, "the connection closed"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Writes one message as a frame.
pub async fn write_frame<W, T>(writer: &mut W, message: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(message).map_err(|e| FrameError::Json(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized {
            claimed: body.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    let len = u32::try_from(body.len()).map_err(|_| FrameError::Oversized {
        claimed: body.len(),
        max: MAX_FRAME_BYTES,
    })?;
    writer
        .write_all(&len.to_le_bytes())
        .await
        .map_err(FrameError::Io)?;
    writer.write_all(&body).await.map_err(FrameError::Io)?;
    writer.flush().await.map_err(FrameError::Io)
}

/// Reads one frame. A claimed length over the limit is refused before anything is allocated for it.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let claimed = u32::from_le_bytes(len) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized {
            claimed,
            max: MAX_FRAME_BYTES,
        });
    }
    let mut body = vec![0u8; claimed];
    match reader.read_exact(&mut body).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(FrameError::Io(e)),
    }
    serde_json::from_slice(&body).map_err(|e| FrameError::Json(e.to_string()))
}

/// The client's opening message.
#[must_use]
pub fn client_hello(wants: &[&str]) -> ClientHello {
    ClientHello {
        generation: ENDPOINT_GENERATION,
        protocol_version: PROTOCOL_VERSION,
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities: wants.iter().map(|c| (*c).to_owned()).collect(),
    }
}

/// The server's answer to a client's opening message.
#[must_use]
pub fn welcome_for(hello: &ClientHello) -> ServerWelcome {
    let error = (hello.generation != ENDPOINT_GENERATION).then(|| HandshakeError {
        code: "generation".to_owned(),
        message: format!(
            "this server speaks endpoint generation {ENDPOINT_GENERATION}, the client {}; update this machine",
            hello.generation
        ),
    });
    ServerWelcome {
        generation: ENDPOINT_GENERATION,
        protocol_version: PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities: server_capabilities(),
        error,
    }
}

/// What the client may use of a server, after the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    pub server_version: String,
    pub capabilities: Vec<String>,
}

impl Negotiated {
    #[must_use]
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

/// Checks a welcome. A saved machine needs `surface_interest` and `health_check` (the client cannot tell a dead
/// link from a quiet one without it); a server without them "needs a server update".
pub fn accept_welcome(welcome: ServerWelcome) -> Result<Negotiated, String> {
    if let Some(e) = welcome.error {
        return Err(format!("handshake rejected: {}", e.message));
    }
    if welcome.generation != ENDPOINT_GENERATION {
        return Err(
            "handshake failed: this machine has no compatible server; install or update it"
                .to_owned(),
        );
    }
    let negotiated = Negotiated {
        server_version: welcome.server_version,
        capabilities: welcome.capabilities,
    };
    for needed in [CAP_SURFACE_INTEREST, CAP_HEALTH_CHECK] {
        if !negotiated.has(needed) {
            return Err(
                "this machine needs a server update before it can be used here (protocol: missing a capability)"
                    .to_owned(),
            );
        }
    }
    Ok(negotiated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_message_survives_the_frame_and_the_framing_is_length_prefixed_json() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_frame(&mut a, &ClientMessage::Ping { id: 7 })
            .await
            .expect("writes");
        let back: ClientMessage = read_frame(&mut b).await.expect("reads");
        assert_eq!(back, ClientMessage::Ping { id: 7 });

        let mut raw = Vec::new();
        write_frame(&mut raw, &ServerMessage::Pong { id: 1 })
            .await
            .expect("writes");
        let len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        assert_eq!(len, raw.len() - 4);
        assert_eq!(&raw[4..], br#"{"type":"pong","id":1}"#);
    }

    #[tokio::test]
    async fn an_oversized_claim_is_refused_before_it_is_read_and_a_cut_stream_is_closed() {
        let mut huge = Vec::new();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut reader = huge.as_slice();
        let err = read_frame::<_, ClientMessage>(&mut reader)
            .await
            .expect_err("refused");
        assert!(matches!(err, FrameError::Oversized { .. }));

        let mut cut: &[u8] = &[10, 0, 0, 0, b'{'];
        let err = read_frame::<_, ClientMessage>(&mut cut)
            .await
            .expect_err("cut");
        assert!(matches!(err, FrameError::Closed));
        let mut empty: &[u8] = &[];
        assert!(matches!(
            read_frame::<_, ClientMessage>(&mut empty).await,
            Err(FrameError::Closed)
        ));
    }

    #[tokio::test]
    async fn a_message_from_a_newer_build_is_ignored_not_fatal() {
        let body = br#"{"type":"something_from_next_year","x":1}"#;
        let mut framed = (body.len() as u32).to_le_bytes().to_vec();
        framed.extend_from_slice(body);
        let mut reader = framed.as_slice();
        let m: ServerMessage = read_frame(&mut reader).await.expect("reads");
        assert_eq!(m, ServerMessage::Unknown);
    }

    #[test]
    fn the_handshake_compares_generations_and_capabilities_not_builds() {
        let hello = client_hello(&[CAP_SURFACE_INTEREST]);
        let welcome = welcome_for(&hello);
        let ok = accept_welcome(welcome.clone()).expect("compatible");
        assert!(ok.has(CAP_AGENT_RUNS));

        let mut old_client = hello.clone();
        old_client.generation = 0;
        assert!(
            accept_welcome(welcome_for(&old_client))
                .unwrap_err()
                .contains("rejected")
        );

        let mut weak = welcome;
        weak.capabilities.retain(|c| c != CAP_HEALTH_CHECK);
        assert!(accept_welcome(weak).unwrap_err().contains("update"));
    }
}
