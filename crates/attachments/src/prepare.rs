//! Preparing one submitted image for storage, and reading a stored one back. Preparing touches no storage: a batch
//! is prepared in full before any member is written, so a refused member leaves nothing behind.

use std::fs;

use vivianne_core::attachment::{AttachmentId, Dimensions, ImageRef, MediaType};

use crate::error::{AttachmentError, Code, Result, fail};
use crate::image::detect::{DecodeLimits, decode, probe};
use crate::image::normalize::{Policy, normalize};
use crate::layout::{Layout, checked_digest};
use crate::limiter::Cancel;
use crate::names::image_display_name;
use crate::objects::{digest_bytes, publish_object};

/// Limits on what may be submitted, before anything is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_image_bytes: u64,
    pub max_images_per_message: usize,
    pub max_message_image_bytes: u64,
    pub max_image_pixels: u64,
    /// Largest width and largest height of one image.
    pub max_image_dimension: u32,
    pub media_types: Vec<MediaType>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_image_bytes: 50 * 1024 * 1024,
            max_images_per_message: 20,
            max_message_image_bytes: 200 * 1024 * 1024,
            max_image_pixels: 64_000_000,
            // The largest side a JPEG can carry: by default the pixel limit alone bounds admission.
            max_image_dimension: 65_535,
            media_types: MediaType::ALL.to_vec(),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self { max_pixels: 2048 * 2048, max_dimension: 8192, max_bytes: 4 * 1024 * 1024 }
    }
}

/// One image to store: its bytes, the type the sender declared, and a display name.
#[derive(Debug, Clone)]
pub struct SaveImage {
    pub data: Vec<u8>,
    pub media_type: MediaType,
    pub name: Option<String>,
}

/// A normalized image ready to publish: bytes whose digest is the reference's id.
#[derive(Debug)]
pub struct Prepared {
    pub data: Vec<u8>,
    pub image: ImageRef,
}

/// Decodes, normalizes and verifies one submitted image.
pub fn prepare_image(input: &SaveImage, limits: &Limits, policy: &Policy) -> Result<Prepared> {
    if input.data.len() as u64 > limits.max_image_bytes {
        return fail(Code::ImageTooLarge, "Image exceeds the configured byte limit.");
    }
    if input.data.is_empty() {
        return fail(Code::InvalidImage, "Image is empty.");
    }
    let decoded = decode(
        &input.data,
        Some(DecodeLimits { max_pixels: limits.max_image_pixels, max_dimension: limits.max_image_dimension }),
    )?;
    if decoded.facts.media_type != input.media_type {
        return fail(Code::ImageTypeMismatch, "Declared image type does not match its bytes.");
    }
    let normalized = normalize(&input.data, &decoded, policy)?;
    let id = AttachmentId::from_digest(&digest_bytes(&normalized.data))
        .ok_or_else(|| AttachmentError::new(Code::AttachmentCorrupt, "Digest is malformed."))?;
    let shrunk = (decoded.facts.width, decoded.facts.height) != (normalized.width, normalized.height);
    Ok(Prepared {
        image: ImageRef {
            attachment_id: id,
            media_type: normalized.media_type,
            bytes: normalized.data.len() as u64,
            width: normalized.width,
            height: normalized.height,
            name: image_display_name(input.name.as_deref()),
            original_dimensions: shrunk.then_some(Dimensions { width: decoded.facts.width, height: decoded.facts.height }),
        },
        data: normalized.data,
    })
}

/// Publishes a prepared image, after checking that its bytes still are what its reference says.
pub fn commit_prepared(layout: &Layout, prepared: &Prepared) -> Result<ImageRef> {
    let digest = checked_digest(&prepared.image.attachment_id)?;
    if digest_bytes(&prepared.data) != digest || prepared.data.len() as u64 != prepared.image.bytes {
        return fail(Code::AttachmentCorrupt, "Prepared attachment bytes do not match their reference.");
    }
    let target = layout.image_path(&prepared.image)?;
    publish_object(&layout.root, &layout.home, &target, &prepared.data, digest)?;
    Ok(prepared.image.clone())
}

/// A stored image, verified against its reference.
#[derive(Debug)]
pub struct StoredImage {
    pub image: ImageRef,
    pub data: Vec<u8>,
}

/// Reads a stored image and checks it against its reference: digest, type, length and size. A missing, damaged or
/// swapped object fails; nothing is repaired or guessed.
pub fn read_image(layout: &Layout, image: &ImageRef, cancel: &Cancel) -> Result<StoredImage> {
    cancel.check()?;
    let digest = checked_digest(&image.attachment_id)?;
    let path = layout.image_path(image)?;
    let data = fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AttachmentError::new(Code::AttachmentNotFound, "Attachment object is missing.")
        } else {
            AttachmentError::new(Code::AttachmentReadFailed, "Unable to read image attachment.")
        }
    })?;
    cancel.check()?;
    if digest_bytes(&data) != digest {
        return fail(Code::AttachmentCorrupt, "Stored attachment failed integrity verification.");
    }
    // The digest proves these are the bytes admission fully decoded, so a read re-derives only header facts: no
    // decode, no pixel cost on every replay of history.
    let facts = probe(&data)?;
    cancel.check()?;
    if facts.media_type != image.media_type
        || data.len() as u64 != image.bytes
        || (facts.width, facts.height) != (image.width, image.height)
    {
        return fail(Code::AttachmentCorrupt, "Stored attachment metadata does not match its reference.");
    }
    Ok(StoredImage { image: image.clone(), data })
}
