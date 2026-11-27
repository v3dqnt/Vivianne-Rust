//! Collects one output stream: the last `max_bytes` in memory (errors and results are at the end) and, if asked, the
//! whole stream in a private spill file so the head can still be read. Spilling is only an aid: when it fails or the
//! stream outgrows its cap the file is removed and collection goes on in memory.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A spill file: how large the whole stream may grow in it, and the directory it goes in.
#[derive(Debug, Clone)]
pub struct Spill {
    pub max_bytes: u64,
    pub dir: PathBuf,
}

/// What a finished stream came to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Collected {
    pub text: String,
    /// Some of the stream was dropped from memory.
    pub truncated: bool,
    /// Where the whole stream is, when it was kept whole.
    pub spill_path: Option<PathBuf>,
}

/// What has arrived since an earlier read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub text: String,
    pub next_offset: u64,
    /// Output between `from` and what is left in memory was dropped; only the spill file has it.
    pub lossy: bool,
    pub spill_path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Collector {
    max_bytes: usize,
    tail: VecDeque<u8>,
    total: u64,
    dropped: bool,
    label: &'static str,
    spill: Option<Spill>,
    file: Option<(File, PathBuf)>,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A new file nobody could have planted: random-ish name, fails if anything is there, readable by the owner only.
fn private_file(dir: &Path, label: &str) -> std::io::Result<(File, PathBuf)> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let path = dir.join(format!(
        "viv-output-{}-{}-{nanos:x}-{label}.log",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    Ok((options.open(&path)?, path))
}

impl Collector {
    #[must_use]
    pub fn new(max_bytes: usize, label: &'static str, spill: Option<Spill>) -> Self {
        Self {
            max_bytes,
            tail: VecDeque::new(),
            total: 0,
            dropped: false,
            label,
            spill,
            file: None,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        let overflows = self.tail.len() + chunk.len() > self.max_bytes;
        self.total += chunk.len() as u64;
        if self.spill.is_some() && (overflows || self.file.is_some()) {
            self.spill_chunk(chunk);
        }
        self.tail.extend(chunk);
        if self.tail.len() > self.max_bytes {
            let excess = self.tail.len() - self.max_bytes;
            self.tail.drain(..excess);
            self.dropped = true;
        }
    }

    fn spill_chunk(&mut self, chunk: &[u8]) {
        let Some(spill) = &self.spill else { return };
        if self.total > spill.max_bytes {
            self.discard();
            return;
        }
        let written = match &mut self.file {
            Some((file, _)) => file.write_all(chunk),
            None => private_file(&spill.dir, self.label).and_then(|(mut file, path)| {
                // Everything so far is still in memory the first time the cap is passed.
                let (a, b) = self.tail.as_slices();
                let result = file
                    .write_all(a)
                    .and_then(|()| file.write_all(b))
                    .and_then(|()| file.write_all(chunk));
                self.file = Some((file, path));
                result
            }),
        };
        if written.is_err() {
            self.discard();
        }
    }

    /// Stop spilling and remove the file, which can no longer hold the whole stream.
    fn discard(&mut self) {
        self.spill = None;
        if let Some((file, path)) = self.file.take() {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn spill_path(&self) -> Option<PathBuf> {
        self.file.as_ref().map(|(_, path)| path.clone())
    }

    /// Everything pushed since `from` (0 first, then a read's `next_offset`).
    #[must_use]
    pub fn read_from(&self, from: u64) -> Read {
        let window_start = self.total - self.tail.len() as u64;
        let lossy = from < window_start;
        let skip = if lossy {
            0
        } else {
            usize::try_from(from - window_start).unwrap_or(usize::MAX)
        };
        let bytes: Vec<u8> = self.tail.iter().skip(skip).copied().collect();
        Read {
            text: Self::text(&bytes),
            next_offset: self.total,
            lossy,
            spill_path: self.spill_path(),
        }
    }

    /// Closes the spill file once the stream has ended and says what was kept.
    #[must_use]
    pub fn finish(&mut self) -> Collected {
        let intact = match &mut self.file {
            Some((file, _)) => file.flush().is_ok(),
            None => false,
        };
        if !intact {
            self.discard();
        }
        let bytes: Vec<u8> = self.tail.iter().copied().collect();
        Collected {
            text: Self::text(&bytes),
            truncated: self.dropped,
            spill_path: self.spill_path(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-collect-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn the_last_bytes_are_kept_exactly_however_the_stream_was_cut() {
        for chunks in [
            &["abcdefghij"][..],
            &["ab", "cdefg", "h", "ij"],
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"],
        ] {
            let mut c = Collector::new(4, "stdout", None);
            for chunk in chunks {
                c.push(chunk.as_bytes());
            }
            let done = c.finish();
            assert_eq!(
                (done.text.as_str(), done.truncated, done.spill_path),
                ("ghij", true, None),
                "{chunks:?}"
            );
        }
        let mut small = Collector::new(10, "stdout", None);
        small.push(b"hi");
        assert!(!small.finish().truncated);
    }

    #[test]
    fn reading_in_steps_returns_each_delta_once_and_says_when_some_was_lost() {
        let mut c = Collector::new(4, "stdout", None);
        c.push(b"ab");
        let first = c.read_from(0);
        assert_eq!(
            (first.text.as_str(), first.next_offset, first.lossy),
            ("ab", 2, false)
        );
        c.push(b"cd");
        assert_eq!(c.read_from(2).text, "cd");
        c.push(b"efghij");
        let late = c.read_from(4);
        assert_eq!(
            (late.text.as_str(), late.lossy, late.next_offset),
            ("ghij", true, 10)
        );
        assert_eq!(c.read_from(10).text, "");
    }

    #[test]
    fn the_spill_file_holds_the_whole_stream_while_it_fits_its_cap() {
        let dir = dir("whole");
        let mut c = Collector::new(
            4,
            "stdout",
            Some(Spill {
                max_bytes: 100,
                dir: dir.clone(),
            }),
        );
        for chunk in ["abc", "def", "ghi"] {
            c.push(chunk.as_bytes());
        }
        let done = c.finish();
        assert_eq!(done.text, "fghi");
        let path = done.spill_path.expect("spilled");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "abcdefghi");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stream_past_the_spill_cap_or_a_missing_directory_leaves_only_the_tail() {
        let dir = dir("cap");
        let mut c = Collector::new(
            2,
            "stderr",
            Some(Spill {
                max_bytes: 5,
                dir: dir.clone(),
            }),
        );
        c.push(b"abcd");
        c.push(b"efgh");
        let done = c.finish();
        assert_eq!((done.text.as_str(), done.spill_path), ("gh", None));
        assert_eq!(
            std::fs::read_dir(&dir).expect("dir").count(),
            0,
            "the partial file is removed"
        );
        let mut gone = Collector::new(
            2,
            "stdout",
            Some(Spill {
                max_bytes: 99,
                dir: dir.join("nowhere"),
            }),
        );
        gone.push(b"abcd");
        assert_eq!(gone.finish().text, "cd");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
