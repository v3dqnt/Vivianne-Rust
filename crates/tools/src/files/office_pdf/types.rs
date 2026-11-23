//! Authorized Office input and complete PDF output for the shared converter: the request, the result, the failure
//! classes, the limits, and the two seams (where the bytes come from, and what converts them).

use std::fmt;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Binary Office and Office Open XML formats the converter accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extension {
    Doc,
    Docx,
    Xls,
    Xlsx,
    Ppt,
    Pptx,
}

impl Extension {
    /// The extension of a path, case-insensitively; `None` for anything that is not an Office file.
    #[must_use]
    pub fn of_path(path: &str) -> Option<Self> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (_, ext) = name.rsplit_once('.')?;
        Some(match ext.to_ascii_lowercase().as_str() {
            "doc" => Self::Doc,
            "docx" => Self::Docx,
            "xls" => Self::Xls,
            "xlsx" => Self::Xlsx,
            "ppt" => Self::Ppt,
            "pptx" => Self::Pptx,
            _ => return None,
        })
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Doc => "doc",
            Self::Docx => "docx",
            Self::Xls => "xls",
            Self::Xlsx => "xlsx",
            Self::Ppt => "ppt",
            Self::Pptx => "pptx",
        }
    }
}

/// Foreground previews and explicit questions go ahead of speculative background conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Foreground,
    Background,
}

/// Why a conversion failed, in terms a consumer can show without engine diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    InputTooLarge,
    OutputTooLarge,
    InvalidDocument,
    UnsupportedFormat,
    InvalidOutput,
    Timeout,
    Unavailable,
    Failed,
    Busy,
    SourceChanged,
    /// The caller stopped waiting (the original rejects with the caller's own reason).
    Cancelled,
}

impl Code {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputTooLarge => "input-too-large",
            Self::OutputTooLarge => "output-too-large",
            Self::InvalidDocument => "invalid-document",
            Self::UnsupportedFormat => "unsupported-format",
            Self::InvalidOutput => "invalid-output",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Busy => "busy",
            Self::SourceChanged => "source-changed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A classified failure; engine details stay in `detail`, which is for logs and never for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: Code,
    pub message: String,
    pub detail: Option<String>,
}

impl Error {
    #[must_use]
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(super) fn busy() -> Self {
        Self::new(
            Code::Busy,
            "The document converter has reached its admission limit.",
        )
    }

    pub(super) fn unavailable() -> Self {
        Self::new(Code::Unavailable, "The document converter is unavailable.")
    }

    pub(super) fn cancelled() -> Self {
        Self::new(Code::Cancelled, "Office conversion cancelled")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Error {}

/// Where an authorized source's bytes come from, read only after the converter admits the job.
#[async_trait]
pub trait Source: Send + Sync {
    /// Reads at most `max_bytes` plus one overflow sentinel byte. Returns the bytes and the version actually read; a
    /// version that differs from the request's rejects the conversion. Stops when `cancel` fires.
    async fn read(
        &self,
        cancel: &CancellationToken,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, String), Error>;
}

/// Source authorization and metadata lookup finish before a request is submitted.
pub struct Request {
    pub extension: Extension,
    pub priority: Priority,
    /// An unambiguous encoding of authorization scope and canonical path, for pre-read deduplication.
    pub source_key: String,
    pub version: String,
    /// The authorized stat size; omitted, the converter reserves its whole input limit.
    pub bytes: Option<usize>,
    pub source: std::sync::Arc<dyn Source>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("extension", &self.extension)
            .field("priority", &self.priority)
            .field("source_key", &self.source_key)
            .field("version", &self.version)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

/// What a converter produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub pdf: Vec<u8>,
    /// Requested OOXML font families the conversion lacked; binary Office formats give an empty list.
    pub missing_fonts: Vec<String>,
}

/// A successful conversion; failed and interrupted ones are errors. Every result owns its buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub pdf: Vec<u8>,
    pub missing_fonts: Vec<String>,
    /// Generation and content identity; consumers must not parse it.
    pub cache_key: String,
    /// One converter lifetime; changes when the engine or its settings are replaced.
    pub generation: String,
}

/// Executes one admitted conversion and settles after its scratch files are gone.
#[async_trait]
pub trait Converter: Send + Sync {
    async fn convert(
        &self,
        bytes: &[u8],
        extension: Extension,
        cancel: &CancellationToken,
    ) -> Result<Converted, Error>;
}

/// Queue, source and cache limits, and the engine's own bounds. Every number must be positive (background may be zero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Simultaneous conversions; queued callers stay cancellable.
    pub max_concurrent_conversions: usize,
    /// Metadata-only jobs waiting for source admission.
    pub max_queued_jobs: usize,
    /// Outstanding conversion readers.
    pub max_readers: usize,
    /// Reserved bytes across admitted source reads and conversions.
    pub max_source_bytes: usize,
    /// Concurrent background jobs; zero refuses speculative work.
    pub max_background_conversions: usize,
    /// Retained content-addressed PDFs.
    pub max_cached_entries: usize,
    /// Retained PDF bytes.
    pub max_cached_bytes: usize,
    /// Retained source-version aliases to cached content.
    pub max_source_entries: usize,
    /// Milliseconds a conversion may run once an engine has it; queue time does not count.
    pub timeout_ms: u64,
    /// Source bytes.
    pub max_input_bytes: usize,
    /// Complete PDF bytes.
    pub max_output_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_concurrent_conversions: 2,
            max_queued_jobs: 8,
            max_readers: 32,
            max_source_bytes: 100 * 1024 * 1024,
            max_background_conversions: 1,
            max_cached_entries: 8,
            max_cached_bytes: 128 * 1024 * 1024,
            max_source_entries: 64,
            timeout_ms: 60_000,
            max_input_bytes: 50 * 1024 * 1024,
            max_output_bytes: 100 * 1024 * 1024,
        }
    }
}

impl Config {
    /// Names the first limit that is not usable.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            (
                "max_concurrent_conversions",
                self.max_concurrent_conversions,
            ),
            ("max_queued_jobs", self.max_queued_jobs),
            ("max_readers", self.max_readers),
            ("max_source_bytes", self.max_source_bytes),
            ("max_cached_entries", self.max_cached_entries),
            ("max_cached_bytes", self.max_cached_bytes),
            ("max_source_entries", self.max_source_entries),
            ("max_input_bytes", self.max_input_bytes),
            ("max_output_bytes", self.max_output_bytes),
        ] {
            if value == 0 {
                return Err(format!("office to pdf: {name} must be positive"));
            }
        }
        if self.timeout_ms == 0 {
            return Err("office to pdf: timeout_ms must be positive".into());
        }
        if self.max_source_bytes < self.max_input_bytes {
            return Err("office to pdf: max_source_bytes must be at least max_input_bytes".into());
        }
        Ok(())
    }
}
