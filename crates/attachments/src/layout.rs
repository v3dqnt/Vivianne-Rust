//! Where things live. Everything sits under one home folder that the caller hands in (the app passes `~/.viv`), so
//! tests use a scratch folder and nothing here ever reads the real one.
//!
//! ```text
//! <home>/attachments/v1/objects/<aa>/<sha>              normalized images
//! <home>/attachments/v1/file-objects/<aa>/<sha>         verbatim files, one per digest
//! <home>/attachments/v1/files/<aa>/<sha>/<name>         the same bytes under their display name
//! <home>/attachments/v1/tmp/                            staging
//! <home>/cache/attachments/request-images/<aa>/<hash>   request-sized versions, safe to delete
//! ```

use std::path::{Path, PathBuf};

use vivianne_core::attachment::{AttachmentId, FileRef, ImageRef};

use crate::error::{Code, Result, fail};
use crate::names::file_leaf_name;

#[derive(Debug, Clone)]
pub struct Layout {
    pub home: PathBuf,
    pub root: PathBuf,
    pub cache: PathBuf,
}

impl Layout {
    #[must_use]
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            root: home.join("attachments").join("v1"),
            cache: home.join("cache").join("attachments"),
        }
    }

    fn shard(&self, folder: &str, digest: &str) -> PathBuf {
        self.root.join(folder).join(&digest[..2])
    }

    /// Where a normalized image is.
    pub fn image_path(&self, image: &ImageRef) -> Result<PathBuf> {
        let digest = checked_digest(&image.attachment_id)?;
        Ok(self.shard("objects", digest).join(digest))
    }

    /// Where a file's bytes are, under its display name: a path whose last part is the real file name.
    pub fn file_path(&self, file: &FileRef) -> Result<PathBuf> {
        let digest = checked_digest(&file.attachment_id)?;
        if file.name != file_leaf_name(Some(&file.name)) {
            return fail(
                Code::InvalidAttachmentRef,
                "File attachment reference is invalid.",
            );
        }
        Ok(self.shard("files", digest).join(digest).join(&file.name))
    }

    /// The one object shared by every display name of a digest.
    #[must_use]
    pub fn file_object_path(&self, digest: &str) -> PathBuf {
        self.shard("file-objects", digest).join(digest)
    }

    /// Where a request-sized version is cached.
    #[must_use]
    pub fn request_image_path(&self, hash: &str) -> PathBuf {
        self.cache
            .join("request-images")
            .join(&hash[..2])
            .join(hash)
    }
}

/// The digest of a reference, or a refusal for an id of the wrong shape. A damaged id is never repaired.
pub fn checked_digest(id: &AttachmentId) -> Result<&str> {
    match AttachmentId::parse(id.as_str()) {
        Some(_) => Ok(id.digest()),
        None => fail(
            Code::InvalidAttachmentRef,
            "Attachment reference is invalid.",
        ),
    }
}
