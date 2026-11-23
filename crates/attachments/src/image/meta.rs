//! What the container of an image says beyond its pixels: descriptive metadata, a colour profile, extra frames. The
//! decoders do not report all of it, so each format's chunks or segments are walked here. Every walk is bounded by
//! the input length and never indexes past it: these bytes come from outside (R3.2).

use vivianne_core::attachment::MediaType;

/// Facts read from a container's structure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Container {
    /// More than one frame.
    pub animated: bool,
    /// Descriptive metadata, a colour profile or an orientation is present.
    pub carries_metadata: bool,
}

/// Walks the structure of `data`, which must be of `media_type` (the caller checked the signature).
#[must_use]
pub fn scan(media_type: MediaType, data: &[u8]) -> Container {
    match media_type {
        MediaType::Png => png(data),
        MediaType::Jpeg => jpeg(data),
        MediaType::Webp => webp(data),
        MediaType::Gif => gif(data),
    }
}

fn be32(data: &[u8], at: usize) -> Option<usize> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn le32(data: &[u8], at: usize) -> Option<usize> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn png(data: &[u8]) -> Container {
    let mut out = Container::default();
    let mut at = 8; // after the signature
    while let (Some(len), Some(kind)) = (be32(data, at), data.get(at + 4..at + 8)) {
        match kind {
            b"acTL" => out.animated = true,
            b"eXIf" | b"iTXt" | b"tEXt" | b"zTXt" | b"iCCP" => out.carries_metadata = true,
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        // length, type, data, crc
        let Some(next) = at.checked_add(12).and_then(|n| n.checked_add(len)) else { break };
        at = next;
    }
    out
}

fn jpeg(data: &[u8]) -> Container {
    let mut out = Container::default();
    let mut at = 2; // after SOI
    while data.get(at) == Some(&0xFF) {
        let Some(&marker) = data.get(at + 1) else { break };
        match marker {
            // Fill bytes before a marker.
            0xFF => {
                at += 1;
                continue;
            }
            // Start of scan: the entropy-coded data follows; no more headers.
            0xDA | 0xD9 => break,
            // Markers with no length.
            0x01 | 0xD0..=0xD8 => {
                at += 2;
                continue;
            }
            // Exif and XMP, ICC profile, Photoshop and IPTC, comment.
            0xE1 | 0xE2 | 0xED | 0xFE => out.carries_metadata = true,
            _ => {}
        }
        let Some(len) = data.get(at + 2..at + 4).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]))) else { break };
        if len < 2 {
            break;
        }
        at += 2 + len;
    }
    out
}

fn webp(data: &[u8]) -> Container {
    let mut out = Container::default();
    let mut at = 12; // RIFF, size, WEBP
    while let (Some(kind), Some(len)) = (data.get(at..at + 4), le32(data, at + 4)) {
        match kind {
            b"EXIF" | b"XMP " | b"ICCP" => out.carries_metadata = true,
            b"VP8X" => out.animated = data.get(at + 8).is_some_and(|flags| flags & 0x02 != 0),
            b"ANIM" | b"ANMF" => out.animated = true,
            _ => {}
        }
        // Chunks are padded to an even size.
        let Some(next) = at.checked_add(8).and_then(|n| n.checked_add(len)).and_then(|n| n.checked_add(len & 1)) else { break };
        at = next;
    }
    out
}

/// Skips the sub-blocks of a GIF extension or of image data: length-prefixed runs ended by a zero length.
fn skip_sub_blocks(data: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = usize::from(*data.get(at)?);
        at += 1;
        if len == 0 {
            return Some(at);
        }
        at = at.checked_add(len)?;
    }
}

fn gif(data: &[u8]) -> Container {
    let mut out = Container::default();
    let mut frames = 0usize;
    let Some(flags) = data.get(10) else { return out };
    let mut at = 13;
    if flags & 0x80 != 0 {
        at += 3 * (1usize << ((flags & 0x07) + 1));
    }
    loop {
        match data.get(at) {
            // Extension: a label, then sub-blocks. A comment carries text.
            Some(0x21) => {
                if data.get(at + 1) == Some(&0xFE) {
                    out.carries_metadata = true;
                }
                let Some(next) = skip_sub_blocks(data, at + 2) else { break };
                at = next;
            }
            // Image: position and size, an optional local palette, a code size, then sub-blocks.
            Some(0x2C) => {
                frames += 1;
                let Some(local) = data.get(at + 9) else { break };
                let mut next = at + 10;
                if local & 0x80 != 0 {
                    next += 3 * (1usize << ((local & 0x07) + 1));
                }
                let Some(after) = skip_sub_blocks(data, next + 1) else { break };
                at = after;
            }
            _ => break,
        }
    }
    out.animated = frames > 1;
    out
}
