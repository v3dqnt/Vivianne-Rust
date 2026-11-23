//! The wire protocol of a paired connection: one long-lived stream, carrying length-prefixed JSON frames. The
//! protocol, its ALPN and its frames are the old app's (version 2), so a pairing between this app and the old one
//! works. Frames this app has no use for (the old app's mirrored session messages) are read and dropped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::collab::ActivityEntry;

/// Sent in `Hello`, so two versions that do not match say so instead of failing on a frame.
pub const PROTOCOL_VERSION: u32 = 2;

pub const ALPN: &[u8] = b"vivianne/multiplayer/2";

/// What one side spent on the project in a day.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WireMessage {
    /// The first frame either side sends.
    #[serde(rename_all = "camelCase")]
    Hello {
        protocol_version: u32,
        display_name: Option<String>,
    },
    /// One message of the partner's agent. Not used by this app: read, then dropped.
    SessionMessage { message: serde_json::Value },
    /// The people talking, on the same connection.
    Chat { text: String },
    /// What one of the sender's runs did, in a few lines.
    Activity { entry: Box<ActivityEntry> },
    /// The sender's daily usage on the shared project.
    Usage { days: BTreeMap<String, DayUsage> },
    /// Commits the receiver lacks, as a base64 git bundle (when there is no shared remote).
    GitBundle {
        base: Option<String>,
        head: String,
        data: String,
    },
    /// The sender's HEAD, so the receiver bundles only what is new.
    GitHead { head: String },
}

/// Real frames (chat, a run's summary, a bundle) are a few MB at most; the cap limits what a buggy or hostile
/// partner can make this side hold.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Writes one frame.
///
/// # Errors
/// The frame is too large, or the stream failed.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    stream: &mut W,
    msg: &WireMessage,
) -> Result<(), String> {
    let body =
        serde_json::to_vec(msg).map_err(|e| format!("couldn't encode a multiplayer frame: {e}"))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(format!(
            "multiplayer frame too large ({} bytes)",
            body.len()
        ));
    }
    let len = u32::try_from(body.len()).map_err(|_| "multiplayer frame too large".to_owned())?;
    stream
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| format!("multiplayer write failed: {e}"))?;
    stream
        .write_all(&body)
        .await
        .map_err(|e| format!("multiplayer write failed: {e}"))?;
    Ok(())
}

/// Reads one frame. `None` when the partner closed the stream cleanly.
///
/// The body is read in chunks as it arrives instead of allocating what the length prefix claims: a peer can claim
/// any length and then send nothing, so memory tracks what was actually received.
///
/// # Errors
/// The stream failed mid-frame, the frame is too large, or it is not a frame.
pub async fn read_frame<R: AsyncRead + Unpin>(
    stream: &mut R,
) -> Result<Option<WireMessage>, String> {
    let mut len_bytes = [0_u8; 4];
    match stream.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(format!("multiplayer read failed: {e}")),
    }
    let len = usize::try_from(u32::from_be_bytes(len_bytes)).unwrap_or(usize::MAX);
    if len > MAX_FRAME_BYTES {
        return Err(format!("the partner sent an oversized frame ({len} bytes)"));
    }
    const CHUNK: usize = 64 * 1024;
    let mut body = Vec::with_capacity(len.min(CHUNK));
    let mut chunk = vec![0_u8; CHUNK];
    while body.len() < len {
        let want = (len - body.len()).min(CHUNK);
        stream
            .read_exact(&mut chunk[..want])
            .await
            .map_err(|e| format!("multiplayer read failed: {e}"))?;
        body.extend_from_slice(&chunk[..want]);
    }
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| format!("couldn't decode a multiplayer frame: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_a_closed_stream_reads_as_none() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, &WireMessage::Chat { text: "hi".into() })
            .await
            .expect("writes");
        write_frame(
            &mut a,
            &WireMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                display_name: Some("A".into()),
            },
        )
        .await
        .expect("writes");
        drop(a);
        assert!(
            matches!(read_frame(&mut b).await, Ok(Some(WireMessage::Chat { text })) if text == "hi")
        );
        assert!(matches!(
            read_frame(&mut b).await,
            Ok(Some(WireMessage::Hello {
                protocol_version: 2,
                ..
            }))
        ));
        assert!(matches!(read_frame(&mut b).await, Ok(None)));
    }

    #[tokio::test]
    async fn a_frame_that_claims_more_than_the_cap_is_refused_before_anything_is_allocated() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_be_bytes()).await.expect("writes");
        assert!(read_frame(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn the_old_apps_frames_are_understood() {
        let old = br#"{"type":"hello","protocolVersion":2,"displayName":"Old"}"#;
        let mut wire = u32::try_from(old.len())
            .expect("small")
            .to_be_bytes()
            .to_vec();
        wire.extend_from_slice(old);
        let mut reader = wire.as_slice();
        assert!(
            matches!(read_frame(&mut reader).await, Ok(Some(WireMessage::Hello { display_name: Some(n), .. })) if n == "Old")
        );
        let session = br#"{"type":"sessionMessage","message":{"role":"user"}}"#;
        let mut wire = u32::try_from(session.len())
            .expect("small")
            .to_be_bytes()
            .to_vec();
        wire.extend_from_slice(session);
        let mut reader = wire.as_slice();
        assert!(matches!(
            read_frame(&mut reader).await,
            Ok(Some(WireMessage::SessionMessage { .. }))
        ));
    }
}
