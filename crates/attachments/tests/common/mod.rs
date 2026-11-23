//! Shared helpers for the attachment tests: a scratch home that cleans up read-only objects, and images built in code.
//! No test reads the real `~/.viv` (R7.3).

#![allow(dead_code)]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::gif::{GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{Delay, ExtendedColorType, Frame, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};
use vivianne_attachments::{Config, Store};

/// A folder under the system temp folder, removed (read-only objects included) when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("viv-attach-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn store(&self, config: Config) -> Store {
        Store::new(&self.0, config).expect("valid config")
    }
}

fn make_tree_writable(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    let mut permissions = meta.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    let _ = std::fs::set_permissions(path, permissions);
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                make_tree_writable(&entry.path());
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        make_tree_writable(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A smooth colour gradient: compresses well.
pub fn gradient(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| Rgb([(x * 255 / width.max(1)) as u8, (y * 255 / height.max(1)) as u8, 128]))
}

/// Pixels that do not compress: a stand-in for a photograph.
pub fn noise(width: u32, height: u32) -> RgbImage {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    RgbImage::from_fn(width, height, |_, _| {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let b = state.to_be_bytes();
        Rgb([b[0], b[2], b[4]])
    })
}

pub fn png(img: &RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgb8).expect("png");
    out
}

pub fn png_rgba(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8).expect("png");
    out
}

/// A 16-bit PNG.
pub fn png16(width: u32, height: u32) -> Vec<u8> {
    let samples: Vec<u8> = (0..width * height * 3).flat_map(|i| ((i * 257) as u16).to_be_bytes()).collect();
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(&samples, width, height, ExtendedColorType::Rgb16).expect("png16");
    out
}

pub fn jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgb8)
        .expect("jpeg");
    out
}

/// A still WebP (lossless).
pub fn webp(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .expect("webp");
    out
}

/// A GIF of `frames` frames, each a solid colour.
pub fn gif(width: u32, height: u32, frames: u8) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut out);
        encoder.set_repeat(Repeat::Infinite).expect("repeat");
        for i in 0..frames {
            let img = RgbaImage::from_pixel(width, height, Rgba([i.wrapping_mul(60), 40, 200, 255]));
            encoder.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1))).expect("frame");
        }
    }
    out
}

/// Inserts an APP1 Exif segment holding only an orientation right after a JPEG's SOI.
pub fn jpeg_with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = b"MM\0\x2A\0\0\0\x08".to_vec();
    tiff.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1]);
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut segment = b"Exif\0\0".to_vec();
    segment.extend_from_slice(&tiff);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((segment.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&segment);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Inserts a PNG text chunk before the first IDAT.
pub fn png_with_text(png: &[u8]) -> Vec<u8> {
    let at = png.windows(4).position(|w| w == b"IDAT").expect("idat") - 4;
    let body = b"Comment\0made by a test";
    let mut chunk = (body.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(body);
    chunk.extend_from_slice(&[0, 0, 0, 0]); // the scanner does not check the checksum
    let mut out = png[..at].to_vec();
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&png[at..]);
    out
}

pub fn dimensions_of(data: &[u8]) -> (u32, u32) {
    image::ImageReader::new(Cursor::new(data)).with_guessed_format().expect("format").into_dimensions().expect("size")
}
