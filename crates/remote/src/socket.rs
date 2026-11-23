//! The local socket a machine's server listens on and its bridge connects to: a Unix socket (mode 0600) on Linux and
//! macOS, a named pipe on Windows. A session has its own, so one machine can run several servers.

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// One accepted or connected stream, the same to the code above whatever it is underneath.
pub struct Stream(Box<dyn Duplex>);

trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stream")
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

/// The session in use: the one asked for, else `VIV_SESSION`, else `default`.
#[must_use]
pub fn resolve_session(asked: Option<&str>) -> String {
    asked
        .map(str::to_owned)
        .or_else(|| std::env::var("VIV_SESSION").ok())
        .filter(|s| crate::catalog::validate_session(s).is_ok())
        .unwrap_or_else(|| crate::catalog::DEFAULT_SESSION.to_owned())
}

/// Where a session's server keeps its files (snapshots, its socket on Unix).
#[must_use]
pub fn session_dir(session: &str) -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["server", "sessions", session])
}

#[cfg(unix)]
mod imp {
    use super::{Stream, io, session_dir};
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
    }

    fn path(session: &str) -> PathBuf {
        session_dir(session).join("viv.sock")
    }

    /// Whether a server answers on this session's socket.
    pub async fn is_listening(session: &str) -> bool {
        UnixStream::connect(path(session)).await.is_ok()
    }

    impl Listener {
        pub fn bind(session: &str) -> io::Result<Self> {
            let path = path(session);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            if path.exists() {
                // A socket nobody answers on is a leftover of a server that died: remove it. One that answers is
                // a running server, and this one must not take its place.
                match std::os::unix::net::UnixStream::connect(&path) {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "a server is already running",
                        ));
                    }
                    Err(_) => std::fs::remove_file(&path)?,
                }
            }
            let inner = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self { inner, path })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let (stream, _) = self.inner.accept().await?;
            Ok(Stream(Box::new(stream)))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub async fn connect(session: &str) -> io::Result<Stream> {
        Ok(Stream(Box::new(UnixStream::connect(path(session)).await?)))
    }
}

#[cfg(windows)]
mod imp {
    use super::{Stream, io};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    pub struct Listener {
        next: NamedPipeServer,
        name: String,
    }

    /// One pipe per user and session, so two accounts on one machine do not meet.
    fn name(session: &str) -> String {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let clean: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
        format!(r"\\.\pipe\viv-server-{clean}-{session}")
    }

    pub async fn is_listening(session: &str) -> bool {
        ClientOptions::new().open(name(session)).is_ok()
    }

    impl Listener {
        pub fn bind(session: &str) -> io::Result<Self> {
            let name = name(session);
            // `first_pipe_instance` fails when a server already owns the name.
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)
                .map_err(|e| {
                    if e.kind() == io::ErrorKind::PermissionDenied {
                        io::Error::new(io::ErrorKind::AddrInUse, "a server is already running")
                    } else {
                        e
                    }
                })?;
            Ok(Self { next, name })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            let ready = std::mem::replace(
                &mut self.next,
                ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&self.name)?,
            );
            Ok(Stream(Box::new(ready)))
        }
    }

    pub async fn connect(session: &str) -> io::Result<Stream> {
        let name = name(session);
        // A busy pipe (all instances connected for a moment) is retried briefly.
        for _ in 0..20 {
            match ClientOptions::new().open(&name) {
                Ok(client) => return Ok(Stream(Box::new(client))),
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the server is busy",
        ))
    }
}

pub use imp::{Listener, connect, is_listening};

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[tokio::test]
    async fn a_session_has_one_server_and_a_client_reaches_it() {
        let session = format!("t{}", uuid::Uuid::new_v4().simple());
        assert!(!is_listening(&session).await);
        let mut listener = Listener::bind(&session).expect("binds");
        assert!(
            Listener::bind(&session).is_err(),
            "a second server on one session is refused"
        );
        let server = tokio::spawn(async move {
            let mut s = listener.accept().await.expect("accepts");
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.expect("reads");
            s.write_all(&buf).await.expect("echoes");
            s.flush().await.expect("flushes");
            // Held so the client can read before the listener goes away.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });
        let mut c = connect(&session).await.expect("connects");
        c.write_all(b"ping").await.expect("writes");
        let mut back = [0u8; 4];
        c.read_exact(&mut back).await.expect("reads");
        assert_eq!(&back, b"ping");
        server.await.expect("server ends");
    }

    #[test]
    fn the_session_is_the_one_asked_for_or_the_default_and_never_an_unsafe_name() {
        assert_eq!(resolve_session(Some("agents")), "agents");
        assert_eq!(resolve_session(Some("../x")), "default");
    }
}
