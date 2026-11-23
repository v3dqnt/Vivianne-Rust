//! The stored form of an image: one still frame, 8 bits per sample, metadata removed, rotated upright, and within the
//! size budget, so what rides every later request is bounded and the same whatever the source was. A source that is
//! already in that form is kept byte for byte.

use image::imageops::FilterType;
use vivianne_core::attachment::{Dimensions, MediaType};

use super::detect::{Decoded, Detected, decode};
use super::encode::{ladder, first_within_limit};
use crate::error::{AttachmentError, Code, Result, fail};
use crate::geometry::request_image_dimensions;

/// The deployment's limits on the stored image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Total-pixel budget: a larger source is shrunk in proportion.
    pub max_pixels: u64,
    /// Long-edge cap applied after the budget, which bounds extreme aspect ratios.
    pub max_dimension: u32,
    /// Encoded-byte target. When no quality fits, the smallest output is kept.
    pub max_bytes: u64,
}

/// Normalized bytes with the facts a reference records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub data: Vec<u8>,
    pub media_type: MediaType,
    pub width: u32,
    pub height: u32,
}

/// Whether the bytes already are in the stored form.
#[must_use]
pub fn can_pass_through(facts: &Detected, bytes: u64, policy: &Policy) -> bool {
    facts.media_type != MediaType::Gif
        && !facts.animated
        && !facts.carries_metadata
        && !facts.high_depth
        && bytes <= policy.max_bytes
        && u64::from(facts.width) * u64::from(facts.height) <= policy.max_pixels
        && facts.width.max(facts.height) <= policy.max_dimension
}

/// A source with an alpha plane may be encoded without one when every pixel is opaque, but an encoder must never add
/// or drop transparency otherwise. `None`: the source's frame is unspecified (a GIF).
#[must_use]
pub fn alpha_is_compatible(source_has_alpha: Option<bool>, media_type: MediaType, output_has_alpha: bool) -> bool {
    source_has_alpha.is_none_or(|s| s == output_has_alpha || (s && !output_has_alpha && media_type == MediaType::Webp))
}

/// Dimensions under the pixel budget, then under the long-edge cap, with the aspect ratio kept.
fn initial_dimensions(facts: &Detected, policy: &Policy) -> Dimensions {
    let budgeted = request_image_dimensions(facts.width, facts.height, policy.max_pixels);
    let long = budgeted.width.max(budgeted.height);
    if long <= policy.max_dimension {
        return budgeted;
    }
    let scale = f64::from(policy.max_dimension) / f64::from(long);
    Dimensions {
        width: ((f64::from(budgeted.width) * scale).floor() as u32).max(1),
        height: ((f64::from(budgeted.height) * scale).floor() as u32).max(1),
    }
}

/// Checks that an output really is a single-frame, 8-bit, metadata-free image with the facts its encoder reported.
fn verify(out: &Normalized, expected_alpha: Option<bool>) -> Result<()> {
    let seen = decode(&out.data, None)?;
    let f = seen.facts;
    if f.media_type != out.media_type
        || f.width != out.width
        || f.height != out.height
        || f.animated
        || f.carries_metadata
        || f.high_depth
        || !alpha_is_compatible(expected_alpha, f.media_type, f.has_alpha)
    {
        return fail(
            Code::AttachmentWriteFailed,
            "Image normalization did not produce a single-frame 8-bit image with matching metadata.",
        );
    }
    Ok(())
}

/// Produces the stored form of one decoded source.
pub fn normalize(data: &[u8], decoded: &Decoded, policy: &Policy) -> Result<Normalized> {
    let facts = decoded.facts;
    if can_pass_through(&facts, data.len() as u64, policy) {
        return Ok(Normalized { data: data.to_vec(), media_type: facts.media_type, width: facts.width, height: facts.height });
    }
    let target = initial_dimensions(&facts, policy);
    let sized;
    let prepared = if (target.width, target.height) == (facts.width, facts.height) {
        &decoded.pixels
    } else {
        sized = decoded.pixels.resize_exact(target.width, target.height, FilterType::Lanczos3);
        &sized
    };
    let chosen = first_within_limit(ladder(prepared, facts.has_alpha), policy.max_bytes).map_err(|e| match e {
        crate::error::Error::Failed(_) => AttachmentError::new(
            Code::AttachmentWriteFailed,
            "The image could not be converted to the normalized 8-bit form.",
        )
        .into(),
        other => other,
    })?;
    let encoded = chosen.into_inner();
    let out = Normalized { data: encoded.data, media_type: encoded.media_type, width: encoded.width, height: encoded.height };
    verify(&out, (facts.media_type != MediaType::Gif).then_some(facts.has_alpha))?;
    Ok(out)
}
