//! R3.2: every decoder of outside bytes gets hostile input. Nothing here may panic, hang or allocate wildly; a
//! refusal or a harmless answer is the only outcome.

mod common;

use common::{gif, gradient, jpeg, png, png_with_text};
use vivianne_attachments::image::detect::{DecodeLimits, decode, probe};
use vivianne_attachments::image::meta::scan;
use vivianne_core::attachment::MediaType;

fn every_cut(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    // Every prefix of a small file, and every prefix with a hostile tail.
    (0..data.len().min(400)).map(|n| &data[..n])
}

#[test]
fn the_container_scanners_survive_every_truncation_of_every_format() {
    let samples = [
        (MediaType::Png, png_with_text(&png(&gradient(9, 9)))),
        (MediaType::Jpeg, jpeg(&gradient(9, 9), 80)),
        (MediaType::Gif, gif(9, 9, 3)),
        (MediaType::Webp, common::webp(&image::RgbaImage::from_pixel(9, 9, image::Rgba([1, 2, 3, 4])))),
    ];
    for (media_type, data) in &samples {
        for cut in every_cut(data) {
            let _ = scan(*media_type, cut);
        }
        let mut all_ones = data.clone();
        all_ones.iter_mut().skip(8).for_each(|b| *b = 0xFF);
        let _ = scan(*media_type, &all_ones);
        let mut zeros = data.clone();
        zeros.iter_mut().skip(8).for_each(|b| *b = 0);
        let _ = scan(*media_type, &zeros);
    }
}

#[test]
fn a_chunk_that_claims_an_enormous_length_ends_the_walk_without_overflow() {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&u32::MAX.to_be_bytes());
    png.extend_from_slice(b"tEXt");
    assert!(scan(MediaType::Png, &png).carries_metadata);
    let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
    webp.extend_from_slice(b"EXIF");
    webp.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(scan(MediaType::Webp, &webp).carries_metadata);
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFF];
    assert!(scan(MediaType::Jpeg, &jpeg).carries_metadata);
    let mut gif = b"GIF89a\x01\0\x01\0\x00\0\0".to_vec();
    gif.extend_from_slice(&[0x21, 0xFE, 0xFF]);
    assert!(scan(MediaType::Gif, &gif).carries_metadata);
}

#[test]
fn the_scanners_read_what_they_are_meant_to() {
    assert!(scan(MediaType::Png, &png_with_text(&png(&gradient(9, 9)))).carries_metadata);
    assert!(!scan(MediaType::Png, &png(&gradient(9, 9))).carries_metadata);
    assert!(!scan(MediaType::Jpeg, &jpeg(&gradient(9, 9), 80)).carries_metadata);
    assert!(scan(MediaType::Jpeg, &common::jpeg_with_orientation(&jpeg(&gradient(9, 9), 80), 3)).carries_metadata);
    assert!(scan(MediaType::Gif, &gif(9, 9, 3)).animated);
    assert!(!scan(MediaType::Gif, &gif(9, 9, 1)).animated);
}

#[test]
fn decoding_refuses_garbage_truncation_and_wrong_headers_without_panicking() {
    let good = png(&gradient(30, 30));
    let limits = Some(DecodeLimits { max_pixels: 64_000_000, max_dimension: 65_535 });
    for cut in every_cut(&good) {
        let _ = probe(cut);
        let _ = decode(cut, limits);
    }
    for noise in [vec![0u8; 100], vec![0xFFu8; 100], b"\x89PNG\r\n\x1a\n".to_vec(), b"GIF89a".to_vec(), b"RIFF\0\0\0\0WEBP".to_vec(), vec![0xFF, 0xD8, 0xFF]] {
        assert!(decode(&noise, limits).is_err(), "{noise:?}");
    }
}

#[test]
fn a_header_claiming_a_huge_raster_is_refused_before_it_is_decoded() {
    // A valid PNG header that declares 60000 x 60000 pixels, with no pixel data behind it.
    let mut header = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&60_000u32.to_be_bytes());
    ihdr.extend_from_slice(&60_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    header.extend_from_slice(&13u32.to_be_bytes());
    header.extend_from_slice(b"IHDR");
    header.extend_from_slice(&ihdr);
    header.extend_from_slice(&[0, 0, 0, 0]);
    let limits = Some(DecodeLimits { max_pixels: 64_000_000, max_dimension: 65_535 });
    let start = std::time::Instant::now();
    assert!(decode(&header, limits).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}
