//! The listener. Every request has a size and a time limit, and the number of connections is capped, so no client
//! can hold the service by sending much or little. It speaks plain HTTP: TLS belongs to what is in front of it.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::api::{self, Request};
use crate::sqlite::Database;

pub const MAX_BODY: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 256;
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECTION_TIMEOUT: Duration = Duration::from_mins(1);
const BODY_TIMEOUT: Duration = Duration::from_secs(15);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn respond(status: u16, body: &serde_json::Value) -> hyper::Response<Full<Bytes>> {
    let mut response = hyper::Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() =
        hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    response
}

fn error(status: u16, message: &str) -> hyper::Response<Full<Bytes>> {
    respond(status, &serde_json::json!({ "error": message }))
}

async fn serve_one(
    db: Arc<Database>,
    request: hyper::Request<Incoming>,
) -> hyper::Response<Full<Bytes>> {
    let (head, body) = request.into_parts();
    let read = tokio::time::timeout(BODY_TIMEOUT, Limited::new(body, MAX_BODY).collect());
    let body = match read.await {
        Err(_) => return error(408, "the request took too long to arrive"),
        Ok(Err(_)) => return error(413, "the request body is too large or was cut off"),
        Ok(Ok(collected)) => collected.to_bytes(),
    };
    // The database is synchronous, so it works on a thread that may block.
    let handled = tokio::task::spawn_blocking(move || {
        let header = |name: &str| head.headers.get(name).and_then(|v| v.to_str().ok());
        api::handle(
            &db,
            now_ms(),
            Request {
                method: head.method.as_str(),
                path: head.uri.path(),
                query: head.uri.query().unwrap_or(""),
                bearer: header(AUTHORIZATION.as_str()).and_then(|v| v.strip_prefix("Bearer ")),
                idempotency_key: header("idempotency-key"),
                body: &body,
            },
        )
    });
    match handled.await {
        Ok(response) => respond(response.status, &response.body),
        Err(_) => error(500, "the request could not be handled"),
    }
}

/// Serves until `stop` completes.
pub async fn serve(
    listener: TcpListener,
    db: Arc<Database>,
    stop: impl Future<Output = ()> + Send,
) -> std::io::Result<()> {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    tokio::pin!(stop);
    loop {
        let accepted = tokio::select! {
            () = &mut stop => return Ok(()),
            accepted = listener.accept() => accepted,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            // One bad connection (or running out of handles for a moment) does not end the service.
            Err(e) => {
                eprintln!("viv-hub: accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // Over the cap, the connection is dropped rather than queued without limit.
        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
            continue;
        };
        let db = Arc::clone(&db);
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let db = Arc::clone(&db);
                async move { Ok::<_, Infallible>(serve_one(db, request).await) }
            });
            let connection = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service);
            // A broken or slow client is its own problem; nothing about its request is logged.
            let _ = tokio::time::timeout(CONNECTION_TIMEOUT, connection).await;
            drop(slot);
        });
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;

    use super::*;

    async fn exchange(address: std::net::SocketAddr, request: String) -> String {
        let mut stream = TcpStream::connect(address).await.expect("connects");
        // The server may answer and close before all of an oversized body is written.
        let _ = stream.write_all(request.as_bytes()).await;
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer).await;
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn it_answers_over_a_socket_and_refuses_a_huge_body() {
        let db = Arc::new(Database::in_memory().expect("opens"));
        let token = db
            .issue_token(&"ann".into(), now_ms(), 60_000)
            .expect("issued");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(serve(listener, db, std::future::pending()));

        let me = |auth: &str| {
            format!("GET /v1/me HTTP/1.1\r\nHost: x\r\n{auth}Connection: close\r\n\r\n")
        };
        let signed = exchange(address, me(&format!("Authorization: Bearer {token}\r\n"))).await;
        assert!(
            signed.starts_with("HTTP/1.1 200") && signed.contains("\"ann\""),
            "{signed}"
        );
        let unsigned = exchange(address, me("")).await;
        assert!(unsigned.starts_with("HTTP/1.1 401"), "{unsigned}");
        let size = MAX_BODY + 1;
        let huge = format!(
            "POST /v1/projects HTTP/1.1\r\nHost: x\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n{}",
            "x".repeat(size)
        );
        let refused = exchange(address, huge).await;
        // Either the 413 arrives or the closed socket resets first; it is never accepted.
        assert!(
            refused.is_empty() || refused.starts_with("HTTP/1.1 413"),
            "{refused}"
        );
        server.abort();
    }
}
