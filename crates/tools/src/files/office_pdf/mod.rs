//! Office to PDF: converts authorized Office files (DOC, DOCX, XLS, XLSX, PPT, PPTX) to complete, reusable PDFs on this
//! machine. Port of the original `office-to-pdf` provider.
//!
//! Callers submit a request whose source is already authorized (its key, version and optional size) with a deferred, bounded
//! read. The [`queue`] admits metadata before reading anything, shares one conversion between readers of the same bytes,
//! and keeps finished PDFs by content. [`OfficeToPdf::render_file`] is the path-based entry for a file in a folder; the
//! original's browser Remote method has no counterpart here.

#![allow(clippy::doc_markdown, clippy::similar_names)]

mod ordered;
mod output;
mod queue;
mod soffice;
mod state;
pub mod tool;
mod types;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

pub use output::read_pdf;
pub use queue::Queue;
pub use soffice::{EXECUTABLE_ENV, Soffice, find as find_soffice};
pub use types::{
    Code, Config, Converted, Converter, Error, Extension, Output, Priority, Request, Source,
};

/// A file converted by [`OfficeToPdf::render_file`], with the identity of the source it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub path: String,
    pub version: String,
    pub pdf: Vec<u8>,
    pub missing_fonts: Vec<String>,
    pub generation: String,
}

/// One converter lifetime: its engine, queue and retained PDFs.
#[derive(Debug, Clone)]
pub struct OfficeToPdf {
    queue: Queue,
}

impl OfficeToPdf {
    /// A converter over any engine. An error names a limit that is not usable.
    pub fn new(config: Config, engine: Arc<dyn Converter>) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            queue: Queue::new(config, uuid::Uuid::new_v4().to_string(), engine),
        })
    }

    /// A converter over the LibreOffice installed here; `unavailable` when there is none.
    pub fn from_system(config: Config) -> Result<Self, Error> {
        let executable = find_soffice().ok_or_else(|| {
            Error::new(
                Code::Unavailable,
                "LibreOffice is not installed on this machine.",
            )
        })?;
        let engine = Soffice::new(executable, config.timeout_ms, config.max_output_bytes);
        Self::new(config, Arc::new(engine)).map_err(|message| Error::new(Code::Failed, message))
    }

    /// Changes whenever the engine or its settings are replaced.
    #[must_use]
    pub fn generation(&self) -> &str {
        self.queue.generation()
    }

    /// Converts Office bytes without touching the source.
    pub async fn convert(
        &self,
        request: Request,
        cancel: &CancellationToken,
    ) -> Result<Output, Error> {
        self.queue.read(request, cancel).await
    }

    /// Reads and converts one Office file. The path's extension must be an Office one; the file's version is its
    /// modification time and size, so a file that changes while it waits is `source-changed`.
    pub async fn render_file(
        &self,
        path: &Path,
        priority: Priority,
        cancel: &CancellationToken,
    ) -> Result<Rendered, Error> {
        let display = path.to_string_lossy().into_owned();
        let Some(extension) = Extension::of_path(&display) else {
            return Err(Error::new(
                Code::UnsupportedFormat,
                "The path must end in doc, docx, xls, xlsx, ppt, or pptx.",
            ));
        };
        let absolute = std::fs::canonicalize(path).map_err(|e| {
            Error::new(Code::Failed, "The source could not be found.").with_detail(e.to_string())
        })?;
        let (version, size) = stat(&absolute).map_err(|e| {
            Error::new(Code::Failed, "The source could not be read.").with_detail(e)
        })?;
        let source = FileSource {
            path: absolute.clone(),
            version: version.clone(),
        };
        let request = Request {
            extension,
            priority,
            source_key: absolute.to_string_lossy().into_owned(),
            version: version.clone(),
            bytes: usize::try_from(size).ok(),
            source: Arc::new(source),
        };
        let output = self.convert(request, cancel).await?;
        Ok(Rendered {
            path: absolute.to_string_lossy().into_owned(),
            version,
            pdf: output.pdf,
            missing_fonts: output.missing_fonts,
            generation: output.generation,
        })
    }

    /// Cancels every reader and waits for reads, conversions and scratch cleanup.
    pub async fn dispose(&self) {
        self.queue.dispose().await;
    }
}

/// A file's version: modification time and size.
fn stat(path: &Path) -> Result<(String, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Ok((format!("{nanos}:{}", meta.len()), meta.len()))
}

/// A file on disk, read when the converter admits the job and checked against the version it was authorized at.
struct FileSource {
    path: std::path::PathBuf,
    version: String,
}

#[async_trait]
impl Source for FileSource {
    async fn read(
        &self,
        cancel: &CancellationToken,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, String), Error> {
        let (path, version) = (self.path.clone(), self.version.clone());
        let work = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String), Error> {
            use std::io::Read as _;
            let changed = || Error::new(Code::SourceChanged, "The source changed.");
            if stat(&path).map_err(|_| changed())?.0 != version {
                return Err(changed());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .and_then(|f| f.take(max_bytes as u64 + 1).read_to_end(&mut bytes))
                .map_err(|e| {
                    Error::new(Code::Failed, "The source could not be read.")
                        .with_detail(e.to_string())
                })?;
            if bytes.len() > max_bytes {
                // Whether it grew or was always too big, the class is the one whose cause is the file's identity.
                return Err(if stat(&path).map_err(|_| changed())?.0 == version {
                    Error::new(
                        Code::InputTooLarge,
                        "The source exceeds the reserved byte capacity.",
                    )
                } else {
                    changed()
                });
            }
            if stat(&path).map_err(|_| changed())?.0 != version {
                return Err(changed());
            }
            Ok((bytes, version))
        });
        tokio::select! {
            () = cancel.cancelled() => Err(Error::new(Code::Cancelled, "Office conversion cancelled")),
            joined = work => joined.map_err(|e| Error::new(Code::Failed, "The source could not be read.").with_detail(e.to_string()))?,
        }
    }
}

#[cfg(test)]
mod tests;
