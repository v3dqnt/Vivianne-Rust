//! Request versions: the stored image sized for one model route. A route says how large a picture it takes and how
//! many bytes; the version for that target is made once, cached on disk and reused. Its identity covers every input
//! of the transform, so a changed target or encoder never reuses a stale file. The cache can be deleted at any time:
//! versions are rebuilt from the stored objects.

use std::fs;
use std::path::Path;

use image::imageops::FilterType;
use vivianne_core::attachment::{AttachmentId, ImageRef, MediaType};

use super::detect::{decode, probe};
use super::encode::{QUALITIES, WEBP_METHOD, first_within_limit, ladder};
use super::normalize::alpha_is_compatible;
use crate::error::{Code, Error, Result, fail};
use crate::geometry::long_edge_dimensions;
use crate::limiter::Cancel;
use crate::objects::digest_bytes;

/// Included in every cache key: bump it when the transform changes.
pub const TRANSFORM_VERSION: &str = "viv-request-image-v1";

/// What a route asks for. A target above the source keeps the source size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    /// Encoded-byte target before base64. When no quality fits, the smallest output is kept.
    pub max_bytes: u64,
}

/// One request version with the facts a provider needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestImage {
    /// Cache and upload key: `sha256:` and a digest over every input of the transform.
    pub variant_id: String,
    pub image: ImageRef,
    pub data: Vec<u8>,
    pub media_type: MediaType,
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
}

impl RequestImage {
    /// The picture as standard base64, the form every provider takes.
    #[must_use]
    pub fn to_base64(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }
}

/// Refuses a target with a zero side or byte count.
pub fn validate_target(target: &Target) -> Result<()> {
    if target.width == 0 || target.height == 0 || target.max_bytes == 0 {
        return fail(Code::InvalidAttachmentRef, "Image request target must be positive.");
    }
    Ok(())
}

/// The complete identity of one stored image and one target.
#[must_use]
pub fn variant_id(attachment: &AttachmentId, target: &Target) -> String {
    let descriptor = format!(
        "{{\"transformVersion\":\"{TRANSFORM_VERSION}\",\"attachmentId\":\"{}\",\"targetWidth\":{},\"targetHeight\":{},\"encodedByteBudget\":{},\"qualities\":{QUALITIES:?},\"webpMethod\":{WEBP_METHOD},\"order\":[\"alpha:webp\",\"opaque:jpeg\"]}}",
        attachment.as_str(),
        target.width,
        target.height,
        target.max_bytes
    );
    format!("sha256:{}", digest_bytes(descriptor.as_bytes()))
}

struct Made {
    data: Vec<u8>,
    media_type: MediaType,
    width: u32,
    height: u32,
    has_alpha: bool,
}

/// Reads a cached version if it is still valid for this target. An invalid or unreadable file is just absent.
fn read_cached(path: &Path, target: &Target, source_has_alpha: bool) -> Option<Made> {
    let data = fs::read(path).ok()?;
    let facts = probe(&data).ok()?;
    let fits = facts.width <= target.width && facts.height <= target.height;
    (!facts.high_depth && !facts.animated && fits && alpha_is_compatible(Some(source_has_alpha), facts.media_type, facts.has_alpha))
        .then_some(Made { data, media_type: facts.media_type, width: facts.width, height: facts.height, has_alpha: facts.has_alpha })
}

fn make(stored: &[u8], image: &ImageRef, target: &Target, cancel: &Cancel) -> Result<Made> {
    let decoded = decode(stored, None)?;
    let facts = decoded.facts;
    cancel.check()?;
    if target.width >= image.width && target.height >= image.height && stored.len() as u64 <= target.max_bytes {
        return Ok(Made { data: stored.to_vec(), media_type: image.media_type, width: image.width, height: image.height, has_alpha: facts.has_alpha });
    }
    // Resize by the long edge only, so the short edge is whatever that gives, as the route predicts.
    let long = if image.width >= image.height { target.width } else { target.height };
    let size = long_edge_dimensions(image.width, image.height, long);
    let sized;
    let pixels = if (size.width, size.height) == (image.width, image.height) {
        &decoded.pixels
    } else {
        sized = decoded.pixels.resize_exact(size.width, size.height, FilterType::Lanczos3);
        &sized
    };
    let encoded = first_within_limit(ladder(pixels, facts.has_alpha), target.max_bytes)?.into_inner();
    cancel.check()?;
    let seen = decode(&encoded.data, None)?;
    let f = seen.facts;
    if f.high_depth
        || (f.width, f.height) != (encoded.width, encoded.height)
        || f.media_type != encoded.media_type
        || !alpha_is_compatible(Some(facts.has_alpha), f.media_type, f.has_alpha)
    {
        return fail(Code::AttachmentWriteFailed, "Encoded model-request image does not match its verified metadata.");
    }
    Ok(Made { data: encoded.data, media_type: encoded.media_type, width: encoded.width, height: encoded.height, has_alpha: f.has_alpha })
}

fn write_cached(path: &Path, data: &[u8]) {
    // A cache that cannot be written only costs the next call a rebuild.
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    if fs::write(&temporary, data).is_ok() && fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

/// Makes or reuses the request version of a verified stored image.
pub fn request_version(
    cache_path: impl FnOnce(&str) -> std::path::PathBuf,
    stored: &[u8],
    image: &ImageRef,
    target: &Target,
    cancel: &Cancel,
) -> Result<RequestImage> {
    cancel.check()?;
    validate_target(target)?;
    let source = probe(stored)?;
    let id = variant_id(&image.attachment_id, target);
    let hash = id.strip_prefix("sha256:").unwrap_or(&id).to_owned();
    let path = cache_path(&hash);
    let cached = read_cached(&path, target, source.has_alpha);
    let was_cached = cached.is_some();
    let made = match cached {
        Some(made) => made,
        None => make(stored, image, target, cancel)?,
    };
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !was_cached && made.data != stored {
        write_cached(&path, &made.data);
    }
    Ok(RequestImage {
        variant_id: id,
        image: image.clone(),
        media_type: made.media_type,
        width: made.width,
        height: made.height,
        has_alpha: made.has_alpha,
        data: made.data,
    })
}
