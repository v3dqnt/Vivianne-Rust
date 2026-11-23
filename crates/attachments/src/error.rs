//! Stable failure codes. A caller routes on the code, never on the message, and a message never holds a host path
//! or any bytes.

use std::fmt;

/// Why an attachment operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    TooManyImages,
    ImagesTooLarge,
    UnsupportedImageType,
    InvalidImageBase64,
    InvalidImage,
    ImageTypeMismatch,
    ImageTooLarge,
    ImageTooManyPixels,
    ImageDimensionTooLarge,
    InvalidFileBase64,
    FileTooLarge,
    InvalidAttachmentRef,
    AttachmentCorrupt,
    AttachmentWriteFailed,
    AttachmentNotFound,
    AttachmentReadFailed,
    AttachmentProjectionUnsupported,
    AttachmentFilesUnsupported,
}

impl Code {
    /// The code as written in logs and over the wire: `TOO_MANY_IMAGES`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TooManyImages => "TOO_MANY_IMAGES",
            Self::ImagesTooLarge => "IMAGES_TOO_LARGE",
            Self::UnsupportedImageType => "UNSUPPORTED_IMAGE_TYPE",
            Self::InvalidImageBase64 => "INVALID_IMAGE_BASE64",
            Self::InvalidImage => "INVALID_IMAGE",
            Self::ImageTypeMismatch => "IMAGE_TYPE_MISMATCH",
            Self::ImageTooLarge => "IMAGE_TOO_LARGE",
            Self::ImageTooManyPixels => "IMAGE_TOO_MANY_PIXELS",
            Self::ImageDimensionTooLarge => "IMAGE_DIMENSION_TOO_LARGE",
            Self::InvalidFileBase64 => "INVALID_FILE_BASE64",
            Self::FileTooLarge => "FILE_TOO_LARGE",
            Self::InvalidAttachmentRef => "INVALID_ATTACHMENT_REF",
            Self::AttachmentCorrupt => "ATTACHMENT_CORRUPT",
            Self::AttachmentWriteFailed => "ATTACHMENT_WRITE_FAILED",
            Self::AttachmentNotFound => "ATTACHMENT_NOT_FOUND",
            Self::AttachmentReadFailed => "ATTACHMENT_READ_FAILED",
            Self::AttachmentProjectionUnsupported => "ATTACHMENT_PROJECTION_UNSUPPORTED",
            Self::AttachmentFilesUnsupported => "ATTACHMENT_FILES_UNSUPPORTED",
        }
    }

    /// Whether the person can fix it by changing what they attach, as against a fault of storage.
    #[must_use]
    pub const fn is_admission(self) -> bool {
        matches!(
            self,
            Self::TooManyImages
                | Self::ImagesTooLarge
                | Self::UnsupportedImageType
                | Self::InvalidImageBase64
                | Self::InvalidImage
                | Self::ImageTypeMismatch
                | Self::ImageTooLarge
                | Self::ImageTooManyPixels
                | Self::ImageDimensionTooLarge
                | Self::InvalidFileBase64
                | Self::FileTooLarge
        )
    }
}

/// A failure with a stable code and a readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentError {
    pub code: Code,
    pub message: String,
}

impl AttachmentError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AttachmentError {}

/// An operation that failed, or that its caller cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Failed(AttachmentError),
    Cancelled,
}

impl Error {
    /// The stable code, unless the caller cancelled.
    #[must_use]
    pub fn code(&self) -> Option<Code> {
        match self {
            Self::Failed(e) => Some(e.code),
            Self::Cancelled => None,
        }
    }
}

impl From<AttachmentError> for Error {
    fn from(e: AttachmentError) -> Self {
        Self::Failed(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(e) => e.fmt(f),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A failed result with a code and a message.
pub(crate) fn fail<T>(code: Code, message: impl Into<String>) -> Result<T> {
    Err(AttachmentError::new(code, message).into())
}
