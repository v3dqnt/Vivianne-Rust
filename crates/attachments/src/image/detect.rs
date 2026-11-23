//! Reading an image: a header-only probe for what a stored object must match, and a full decode at admission that
//! also proves the bytes are a whole, valid raster.

use std::io::Cursor;

use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use vivianne_core::attachment::MediaType;

use super::meta;
use crate::error::{Code, Result, fail};

/// Most memory a decoder may ask for, whatever the limits of the deployment say.
const DECODER_MAX_ALLOC: u64 = 1 << 30;

/// What is known about one image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub media_type: MediaType,
    /// Width as a viewer sees it, with the EXIF orientation applied.
    pub width: u32,
    pub height: u32,
    pub animated: bool,
    /// Descriptive metadata, a colour profile or an orientation is present.
    pub carries_metadata: bool,
    /// More than 8 bits per sample.
    pub high_depth: bool,
    pub has_alpha: bool,
}

/// Limits applied to the intrinsic size of a decoded image.
#[derive(Debug, Clone, Copy)]
pub struct DecodeLimits {
    pub max_pixels: u64,
    pub max_dimension: u32,
}

/// A fully decoded image, oriented the way a viewer sees it, beside its facts.
#[derive(Debug)]
pub struct Decoded {
    pub facts: Detected,
    pub pixels: DynamicImage,
}

fn media_type_of(format: ImageFormat) -> Option<MediaType> {
    match format {
        ImageFormat::Png => Some(MediaType::Png),
        ImageFormat::Jpeg => Some(MediaType::Jpeg),
        ImageFormat::WebP => Some(MediaType::Webp),
        ImageFormat::Gif => Some(MediaType::Gif),
        _ => None,
    }
}

fn format_of(media_type: MediaType) -> ImageFormat {
    match media_type {
        MediaType::Png => ImageFormat::Png,
        MediaType::Jpeg => ImageFormat::Jpeg,
        MediaType::Webp => ImageFormat::WebP,
        MediaType::Gif => ImageFormat::Gif,
    }
}

fn invalid() -> crate::error::Error {
    crate::error::AttachmentError::new(Code::InvalidImage, "Unsupported or malformed image data.").into()
}

fn transposes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH
    )
}

fn is_high_depth(color: ColorType) -> bool {
    !matches!(color, ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8)
}

/// The decoder of `data` with the facts that its header and container give. Nothing is decoded yet.
fn open(data: &[u8]) -> Result<(impl ImageDecoder + '_, Detected, Orientation)> {
    let format = image::guess_format(data).map_err(|_| invalid())?;
    let media_type = media_type_of(format).ok_or_else(invalid)?;
    let mut reader = ImageReader::with_format(Cursor::new(data), format_of(media_type));
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(DECODER_MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| invalid())?;
    let orientation = decoder.orientation().map_err(|_| invalid())?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 {
        return Err(invalid());
    }
    let container = meta::scan(media_type, data);
    let color = decoder.color_type();
    let has_profile = decoder.icc_profile().ok().flatten().is_some() || decoder.exif_metadata().ok().flatten().is_some();
    let (width, height) = if transposes(orientation) { (h, w) } else { (w, h) };
    let facts = Detected {
        media_type,
        width,
        height,
        animated: container.animated,
        carries_metadata: container.carries_metadata || has_profile || orientation != Orientation::NoTransforms,
        high_depth: is_high_depth(color),
        has_alpha: color.has_alpha(),
    };
    Ok((decoder, facts, orientation))
}

/// Parses a supported raster's header only. Reads of stored objects use this: admission already decoded these exact
/// bytes completely, so a read only re-derives the facts a reference records.
pub fn probe(data: &[u8]) -> Result<Detected> {
    open(data).map(|(_, facts, _)| facts)
}

/// Fully decodes a supported raster. The size limits are checked from the header, before any pixel is decoded.
pub fn decode(data: &[u8], limits: Option<DecodeLimits>) -> Result<Decoded> {
    let (decoder, mut facts, orientation) = open(data)?;
    if let Some(limits) = limits {
        if u64::from(facts.width) * u64::from(facts.height) > limits.max_pixels {
            return fail(Code::ImageTooManyPixels, "Image exceeds the configured decoded-pixel limit.");
        }
        if facts.width.max(facts.height) > limits.max_dimension {
            return fail(Code::ImageDimensionTooLarge, "Image exceeds the configured per-side pixel limit.");
        }
    }
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(|_| invalid())?;
    pixels.apply_orientation(orientation);
    // A GIF always decodes with an alpha channel; it is transparent only when some pixel is.
    if facts.media_type == MediaType::Gif {
        facts.has_alpha = pixels.to_rgba8().pixels().any(|p| p.0[3] < 255);
    }
    Ok(Decoded { facts, pixels })
}
