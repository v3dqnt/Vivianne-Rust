//! Files stored byte for byte. Nothing is changed or inspected: what was attached is what is stored, under its
//! digest, and also under its display name so a model reading the saved path sees the real file name.

use std::fs::File;
use std::io::Read;

use sha2::{Digest, Sha256};
use vivianne_core::attachment::{AttachmentId, FileRef};

use crate::error::{AttachmentError, Code, Result, fail};
use crate::layout::{Layout, checked_digest};
use crate::limiter::Cancel;
use crate::names::file_leaf_name;
use crate::objects::{publish_alias, publish_staged, stage};

const CHUNK: usize = 1 << 16;

fn reference(digest: &str, name: String, bytes: u64) -> Result<FileRef> {
    let attachment_id = AttachmentId::from_digest(digest).ok_or_else(|| {
        AttachmentError::new(Code::AttachmentCorrupt, "Stored digest is malformed.")
    })?;
    Ok(FileRef {
        attachment_id,
        name,
        bytes,
    })
}

/// Stores `reader`'s bytes verbatim. At most `max_bytes` are accepted (R3.7), counted as they arrive, so an endless
/// source is stopped. Cancelling removes the staging file.
pub fn save_file_stream(
    layout: &Layout,
    reader: &mut dyn Read,
    name: Option<&str>,
    max_bytes: Option<u64>,
    cancel: &Cancel,
) -> Result<FileRef> {
    let name = file_leaf_name(name);
    let staged = stage(&layout.root, &layout.home, reader, max_bytes, cancel)?;
    let object = layout.file_object_path(&staged.sha256);
    publish_staged(&layout.root, &layout.home, &object, &staged)?;
    cancel.check()?;
    let file = reference(&staged.sha256, name, staged.bytes)?;
    publish_alias(
        &layout.root,
        &layout.home,
        &object,
        &layout.file_path(&file)?,
        &staged.sha256,
    )?;
    cancel.check()?;
    Ok(file)
}

/// Stores bytes verbatim.
pub fn save_file(
    layout: &Layout,
    data: &[u8],
    name: Option<&str>,
    max_bytes: Option<u64>,
) -> Result<FileRef> {
    let mut reader = data;
    if max_bytes.is_some_and(|max| data.len() as u64 > max) {
        return fail(
            Code::FileTooLarge,
            "File is larger than the configured size limit.",
        );
    }
    save_file_stream(layout, &mut reader, name, max_bytes, &Cancel::new())
}

/// A stored file read in bounded chunks. The byte count and digest are checked as the last chunk is read: a changed
/// or short file fails that read, so a caller that reached the end without an error holds the exact bytes.
#[derive(Debug)]
pub struct FileChunks {
    file: File,
    hash: Sha256,
    bytes: u64,
    expected_bytes: u64,
    expected_digest: String,
    cancel: Cancel,
    finished: bool,
}

impl Iterator for FileChunks {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if let Err(e) = self.cancel.check() {
            self.finished = true;
            return Some(Err(e));
        }
        let mut buf = vec![0u8; CHUNK];
        match self.file.read(&mut buf) {
            Ok(0) => {
                self.finished = true;
                let digest = crate::objects::digest_from(&self.hash);
                if self.bytes != self.expected_bytes || digest != self.expected_digest {
                    return Some(fail(
                        Code::AttachmentCorrupt,
                        "Stored file attachment failed integrity verification.",
                    ));
                }
                None
            }
            Ok(n) => {
                buf.truncate(n);
                self.hash.update(&buf);
                self.bytes += n as u64;
                Some(Ok(buf))
            }
            Err(_) => {
                self.finished = true;
                Some(fail(
                    Code::AttachmentReadFailed,
                    "Unable to read file attachment.",
                ))
            }
        }
    }
}

/// Opens a stored file for a verified, chunked read.
pub fn read_file_stream(layout: &Layout, file: &FileRef, cancel: &Cancel) -> Result<FileChunks> {
    cancel.check()?;
    let digest = checked_digest(&file.attachment_id)?.to_owned();
    let path = layout.file_path(file)?;
    let opened = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AttachmentError::new(
                Code::AttachmentNotFound,
                "File attachment object is missing.",
            )
        } else {
            AttachmentError::new(
                Code::AttachmentReadFailed,
                "Unable to read file attachment.",
            )
        }
    })?;
    Ok(FileChunks {
        file: opened,
        hash: Sha256::new(),
        bytes: 0,
        expected_bytes: file.bytes,
        expected_digest: digest,
        cancel: cancel.clone(),
        finished: false,
    })
}
