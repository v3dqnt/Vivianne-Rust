//! The bounded output ring behind one job: chunks at absolute byte offsets,
//! head eviction that never moves an assigned offset, and non-consuming reads
//! from any offset.
//!
//! Port of `@deepseek-ai/dsh-jobs-local/ring.ts`.

use super::types::{JobAppendOptions, JobChannel, JobChunk, JobOutputRead};

/// One retained ring entry; `bytes` caches the chunk's UTF-8 length.
#[derive(Debug, Clone)]
struct RingChunk {
    at: u64,
    text: String,
    bytes: usize,
    channel: Option<JobChannel>,
    gap_before: bool,
}

impl RingChunk {
    fn wire(&self) -> JobChunk {
        JobChunk {
            at: self.at,
            text: self.text.clone(),
            channel: self.channel,
            gap_before: self.gap_before,
        }
    }
}

/// The UTF-8-safe tail of `text` no longer than `max_bytes`: the byte cut
/// advances past continuation bytes so the surviving text never starts inside
/// a code point.
fn utf8_tail(text: &str, max_bytes: usize) -> (String, usize) {
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    (tail.to_owned(), tail.len())
}

/// Offsets stay absolute across eviction: `earliest` only ever advances.
#[derive(Debug)]
pub(crate) struct OutputRing {
    /// Retained chunks in offset order.
    chunks: Vec<RingChunk>,
    /// Sum of the retained chunks' byte lengths.
    pub(crate) retained_bytes: usize,
    /// Total UTF-8 bytes ever appended — the offset the next chunk starts at.
    pub(crate) total: u64,
    /// Offset of the oldest retained byte (equals [`Self::total`] when nothing is retained).
    pub(crate) earliest: u64,
}

impl OutputRing {
    pub(crate) fn new() -> Self {
        Self {
            chunks: Vec::new(),
            retained_bytes: 0,
            total: 0,
            earliest: 0,
        }
    }

    /// Append one chunk and trim the head to `cap`.
    ///
    /// Returns false when the chunk was empty and nothing changed.
    pub(crate) fn append(&mut self, text: &str, options: &JobAppendOptions, cap: usize) -> bool {
        if text.is_empty() {
            return false;
        }
        let bytes = text.len();
        self.chunks.push(RingChunk {
            at: self.total,
            text: text.to_owned(),
            bytes,
            channel: options.channel,
            gap_before: options.gap_before,
        });
        self.total += bytes as u64;
        self.retained_bytes += bytes;
        self.trim(cap);
        true
    }

    /// Drop retained head chunks until the ring fits `cap`; a single oversized
    /// chunk keeps only its UTF-8-safe tail with a `gapBefore` marker.
    pub(crate) fn trim(&mut self, cap: usize) {
        while self.retained_bytes > cap && self.chunks.len() > 1 {
            let dropped = self.chunks.remove(0);
            self.retained_bytes -= dropped.bytes;
        }
        if self.chunks.len() == 1 && self.chunks[0].bytes > cap {
            let single = &mut self.chunks[0];
            let (text, bytes) = utf8_tail(&single.text, cap);
            single.at += (single.bytes - bytes) as u64;
            single.text = text;
            single.bytes = bytes;
            single.gap_before = true;
            self.retained_bytes = bytes;
        }
        self.earliest = self.chunks.first().map_or(self.total, |chunk| chunk.at);
    }

    /// Retained chunks overlapping `[from, total)` as fresh wire chunks.
    pub(crate) fn read_from(&self, from: u64) -> JobOutputRead {
        let chunks = self
            .chunks
            .iter()
            .filter(|chunk| chunk.at + chunk.bytes as u64 > from)
            .map(RingChunk::wire)
            .collect();
        JobOutputRead {
            chunks,
            next: self.total,
            lossy: from < self.earliest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> JobAppendOptions {
        JobAppendOptions::default()
    }

    #[test]
    fn empty_append_changes_nothing() {
        let mut ring = OutputRing::new();
        assert!(!ring.append("", &plain(), 1024));
        assert_eq!(ring.total, 0);
        assert_eq!(ring.earliest, 0);
    }

    #[test]
    fn offsets_are_absolute_across_eviction() {
        let mut ring = OutputRing::new();
        ring.append("abc", &plain(), 6);
        ring.append("def", &plain(), 6);
        ring.append("ghi", &plain(), 6);
        assert_eq!(ring.total, 9);
        assert_eq!(ring.earliest, 3);
        let read = ring.read_from(0);
        assert!(read.lossy);
        assert_eq!(read.next, 9);
        assert_eq!(read.chunks.len(), 2);
        assert_eq!(read.chunks[0].text, "def");
        assert_eq!(read.chunks[0].at, 3);
    }

    #[test]
    fn lone_oversized_chunk_keeps_utf8_tail() {
        let mut ring = OutputRing::new();
        ring.append("aé", &plain(), 3);
        assert_eq!(ring.retained_bytes, 3);
        assert_eq!(ring.total, 3);
        assert_eq!(ring.earliest, 0);

        let mut ring = OutputRing::new();
        ring.append("aé", &plain(), 2);
        // "aé" is 3 bytes; a 2-byte tail starts inside the two-byte "é".
        assert_eq!(ring.retained_bytes, 2);
        assert_eq!(ring.earliest, 1);
        let read = ring.read_from(0);
        assert!(read.lossy);
        assert_eq!(read.chunks[0].text, "é");
        assert_eq!(read.chunks[0].at, 1);
        assert!(read.chunks[0].gap_before);
    }

    #[test]
    fn read_from_skips_chunks_ending_at_the_cursor() {
        let mut ring = OutputRing::new();
        ring.append("abc", &plain(), 100);
        ring.append("def", &plain(), 100);
        let read = ring.read_from(3);
        assert!(!read.lossy);
        assert_eq!(read.chunks.len(), 1);
        assert_eq!(read.chunks[0].text, "def");
    }
}
