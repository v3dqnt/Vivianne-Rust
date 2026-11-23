//! The quality ladder shared by normalization and request versions: transparent images are lossy WebP, everything
//! else JPEG, tried from the best quality down until one fits the byte target. Candidates are made lazily, so a
//! target met by the first quality never pays for the others.

use image::{DynamicImage, ExtendedColorType};
use vivianne_core::attachment::MediaType;

use crate::error::{AttachmentError, Code, Result, fail};

/// Spaced so each step buys a real size reduction.
pub const QUALITIES: [u8; 3] = [85, 75, 60];
/// Fastest lossy WebP search: deeper costs several times the encode time for a few percent of size.
pub const WEBP_METHOD: i32 = 0;

/// One encoded picture with its exact facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub data: Vec<u8>,
    pub media_type: MediaType,
    pub width: u32,
    pub height: u32,
}

/// What the ladder chose: the first candidate that fits, or, when none does, the smallest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice<T> {
    Fits(T),
    Exhausted(T),
}

impl<T> Choice<T> {
    pub fn into_inner(self) -> T {
        match self {
            Self::Fits(v) | Self::Exhausted(v) => v,
        }
    }
}

/// Something with an encoded length.
pub trait Candidate {
    fn byte_len(&self) -> usize;
}

impl Candidate for Encoded {
    fn byte_len(&self) -> usize {
        self.data.len()
    }
}

/// Runs `attempts` in order and stops after the first that fits `max_bytes`. When none fits, the smallest wins.
pub fn first_within_limit<T: Candidate>(
    attempts: impl IntoIterator<Item = impl FnOnce() -> Result<T>>,
    max_bytes: u64,
) -> Result<Choice<T>> {
    let mut attempts = attempts.into_iter();
    let Some(first) = attempts.next() else {
        return fail(
            Code::AttachmentWriteFailed,
            "Image encoding needs at least one candidate.",
        );
    };
    let mut smallest = first()?;
    if smallest.byte_len() as u64 <= max_bytes {
        return Ok(Choice::Fits(smallest));
    }
    for attempt in attempts {
        let candidate = attempt()?;
        if candidate.byte_len() as u64 <= max_bytes {
            return Ok(Choice::Fits(candidate));
        }
        if candidate.byte_len() < smallest.byte_len() {
            smallest = candidate;
        }
    }
    Ok(Choice::Exhausted(smallest))
}

fn failed(what: &str) -> crate::error::Error {
    AttachmentError::new(
        Code::AttachmentWriteFailed,
        format!("Image encoding failed: {what}."),
    )
    .into()
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Encoded> {
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut data = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, quality)
        .encode(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|_| failed("jpeg"))?;
    Ok(Encoded {
        data,
        media_type: MediaType::Jpeg,
        width,
        height,
    })
}

fn encode_webp(image: &DynamicImage, quality: u8) -> Result<Encoded> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut config = webp::WebPConfig::new().map_err(|()| failed("webp config"))?;
    config.lossless = 0;
    config.quality = f32::from(quality);
    config.method = WEBP_METHOD;
    let memory = webp::Encoder::from_rgba(rgba.as_raw(), width, height)
        .encode_advanced(&config)
        .map_err(|_| failed("webp"))?;
    Ok(Encoded {
        data: memory.to_vec(),
        media_type: MediaType::Webp,
        width,
        height,
    })
}

/// The ladder for one prepared picture, from the best quality to the lowest.
pub fn ladder(
    image: &DynamicImage,
    has_alpha: bool,
) -> impl Iterator<Item = impl FnOnce() -> Result<Encoded> + '_> {
    QUALITIES.into_iter().map(move |quality| {
        move || {
            if has_alpha {
                encode_webp(image, quality)
            } else {
                encode_jpeg(image, quality)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Fake(usize);

    impl Candidate for Fake {
        fn byte_len(&self) -> usize {
            self.0
        }
    }

    #[test]
    fn later_qualities_are_not_run_after_the_first_that_fits() {
        let ran = Cell::new(0);
        let attempt = |n: usize| {
            let ran = &ran;
            move || {
                ran.set(ran.get() + 1);
                Ok(Fake(n))
            }
        };
        let chosen =
            first_within_limit([attempt(90), attempt(70), attempt(50)], 80).expect("chose");
        assert_eq!(chosen, Choice::Fits(Fake(70)));
        assert_eq!(ran.get(), 2);
        ran.set(0);
        assert_eq!(
            first_within_limit([attempt(10), attempt(5)], 80).expect("chose"),
            Choice::Fits(Fake(10))
        );
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn when_nothing_fits_the_smallest_is_kept() {
        let attempts: Vec<Box<dyn FnOnce() -> Result<Fake>>> = vec![
            Box::new(|| Ok(Fake(900))),
            Box::new(|| Ok(Fake(700))),
            Box::new(|| Ok(Fake(800))),
        ];
        assert_eq!(
            first_within_limit(attempts, 100).expect("chose"),
            Choice::Exhausted(Fake(700))
        );
    }

    #[test]
    fn an_empty_ladder_is_refused() {
        let none: Vec<Box<dyn FnOnce() -> Result<Fake>>> = Vec::new();
        assert!(first_within_limit(none, 1).is_err());
    }

    #[test]
    fn a_failing_candidate_ends_the_ladder_with_its_error() {
        let attempts: Vec<Box<dyn FnOnce() -> Result<Fake>>> = vec![
            Box::new(|| Ok(Fake(900))),
            Box::new(|| fail(Code::AttachmentWriteFailed, "no")),
        ];
        assert!(first_within_limit(attempts, 100).is_err());
    }
}
