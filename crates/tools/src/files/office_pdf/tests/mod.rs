//! The queue and the converter around it, with a made-up engine and made-up sources.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::*;

/// Converts to a small valid PDF named after its input, counting what it did. With a gate it waits for permits.
#[derive(Default)]
struct Fake {
    converts: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    cancelled: AtomicUsize,
    gate: Option<Arc<Semaphore>>,
    fail: std::sync::Mutex<Option<Error>>,
}

impl Fake {
    fn gated() -> (Arc<Self>, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        (
            Arc::new(Self {
                gate: Some(gate.clone()),
                ..Self::default()
            }),
            gate,
        )
    }
}

#[async_trait]
impl Converter for Fake {
    async fn convert(
        &self,
        bytes: &[u8],
        extension: Extension,
        cancel: &CancellationToken,
    ) -> Result<Converted, Error> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        let result = async {
            if let Some(gate) = &self.gate {
                tokio::select! {
                    () = cancel.cancelled() => {
                        self.cancelled.fetch_add(1, Ordering::SeqCst);
                        return Err(Error::new(Code::Cancelled, "cancelled"));
                    }
                    permit = gate.acquire() => permit.map_err(|_| Error::unavailable())?.forget(),
                }
            }
            if let Some(error) = self
                .fail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                return Err(error);
            }
            self.converts.fetch_add(1, Ordering::SeqCst);
            Ok(Converted {
                pdf: format!(
                    "%PDF-1.7\n{}:{}\n%%EOF\n",
                    extension.as_str(),
                    String::from_utf8_lossy(bytes)
                )
                .into_bytes(),
                missing_fonts: vec!["Calibri".into()],
            })
        }
        .await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

/// Bytes in memory, counting reads; may wait for permits before answering and may answer with another version.
struct Mem {
    bytes: Vec<u8>,
    version: String,
    answers_version: Option<String>,
    reads: Arc<AtomicUsize>,
    gate: Option<Arc<Semaphore>>,
}

impl Mem {
    fn new(bytes: &str, version: &str, reads: &Arc<AtomicUsize>) -> Self {
        Self {
            bytes: bytes.as_bytes().to_vec(),
            version: version.into(),
            answers_version: None,
            reads: reads.clone(),
            gate: None,
        }
    }
}

#[async_trait]
impl Source for Mem {
    async fn read(
        &self,
        cancel: &CancellationToken,
        _max_bytes: usize,
    ) -> Result<(Vec<u8>, String), Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            tokio::select! {
                () = cancel.cancelled() => return Err(Error::new(Code::Cancelled, "cancelled")),
                permit = gate.acquire() => permit.map_err(|_| Error::unavailable())?.forget(),
            }
        }
        Ok((
            self.bytes.clone(),
            self.answers_version
                .clone()
                .unwrap_or_else(|| self.version.clone()),
        ))
    }
}

fn request(
    key: &str,
    source: Mem,
    extension: Extension,
    priority: Priority,
    bytes: Option<usize>,
) -> Request {
    Request {
        extension,
        priority,
        source_key: key.into(),
        version: source.version.clone(),
        bytes,
        source: Arc::new(source),
    }
}

fn fg(key: &str, content: &str, reads: &Arc<AtomicUsize>) -> Request {
    request(
        key,
        Mem::new(content, "1", reads),
        Extension::Docx,
        Priority::Foreground,
        Some(content.len()),
    )
}

fn bg(key: &str, content: &str, reads: &Arc<AtomicUsize>) -> Request {
    request(
        key,
        Mem::new(content, "1", reads),
        Extension::Docx,
        Priority::Background,
        Some(content.len()),
    )
}

fn service(config: Config, engine: &Arc<Fake>) -> OfficeToPdf {
    OfficeToPdf::new(config, engine.clone()).expect("a usable configuration")
}

fn reads() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(40)).await;
}

fn count(n: &AtomicUsize) -> usize {
    n.load(Ordering::SeqCst)
}

mod files;
mod queue;
mod tool;
