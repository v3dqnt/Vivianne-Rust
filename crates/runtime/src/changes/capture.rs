//! Whole-file captures around file-tool edits: the content of a path before the turn's first edit of it and at the end,
//! stored content-addressed in the recorder's temporary folder so both sides of a comparison survive later edits without
//! depending on git.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Bytes git inspects for a NUL byte before it treats content as binary.
const BINARY_PROBE_BYTES: usize = 8000;

/// One captured side of a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// No file at the path.
    Absent,
    /// A regular file larger than the cap; its content is not stored.
    Oversized,
    /// A stored copy named by the SHA-256 of its bytes (the original names it by SHA-1).
    File { file: PathBuf, binary: bool },
}

/// Stores the current content of one path. At most `max_bytes + 1` bytes are read, so a file that grows past the cap
/// while it is read costs no more memory than the cap. `Ok(None)` for a path that is neither absent nor a regular file.
pub fn capture_file(
    absolute: &Path,
    directory: &Path,
    max_bytes: usize,
) -> std::io::Result<Option<Capture>> {
    // A folder cannot be opened as a file on every platform, so ask what the path is first.
    match std::fs::metadata(absolute) {
        Ok(meta) if !meta.is_file() => return Ok(None),
        Ok(_) => {}
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Some(Capture::Absent));
        }
        Err(e) => return Err(e),
    }
    let file = match std::fs::File::open(absolute) {
        Ok(file) => file,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => {
            return Ok(Some(Capture::Absent));
        }
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Ok(Some(Capture::Oversized));
    }
    let name = Sha256::digest(&bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{b:02x}");
            hex
        });
    std::fs::create_dir_all(directory)?;
    let stored = directory.join(name);
    // Identical content across paths and turns shares one copy.
    if !stored.exists() {
        std::fs::write(&stored, &bytes)?;
    }
    Ok(Some(Capture::File {
        file: stored,
        binary: bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0),
    }))
}

/// Whether two captures are known to hold the same content. Two absent sides are the same; two stored copies are the
/// same when their bytes hash alike; an oversized side never matches, since its content was not read.
#[must_use]
pub fn same_capture(a: &Capture, b: &Capture) -> bool {
    match (a, b) {
        (Capture::Absent, Capture::Absent) => true,
        (Capture::File { file: x, .. }, Capture::File { file: y, .. }) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-cap-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn identical_content_is_stored_once_and_compares_equal() {
        let d = dir("same");
        std::fs::write(d.join("a"), "hello\n").expect("a");
        std::fs::write(d.join("b"), "hello\n").expect("b");
        let store = d.join("store");
        let a = capture_file(&d.join("a"), &store, 100)
            .expect("io")
            .expect("some");
        let b = capture_file(&d.join("b"), &store, 100)
            .expect("io")
            .expect("some");
        assert!(same_capture(&a, &b));
        assert_eq!(std::fs::read_dir(&store).expect("store").count(), 1);
        std::fs::write(d.join("b"), "other\n").expect("b2");
        assert!(!same_capture(
            &a,
            &capture_file(&d.join("b"), &store, 100)
                .expect("io")
                .expect("some")
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn absent_oversized_binary_and_folders_are_told_apart() {
        let d = dir("kinds");
        let store = d.join("store");
        assert_eq!(
            capture_file(&d.join("nope"), &store, 10).expect("io"),
            Some(Capture::Absent)
        );
        std::fs::write(d.join("big"), "0123456789ab").expect("big");
        assert_eq!(
            capture_file(&d.join("big"), &store, 10).expect("io"),
            Some(Capture::Oversized)
        );
        std::fs::write(d.join("bin"), [1u8, 0, 2]).expect("bin");
        assert!(matches!(
            capture_file(&d.join("bin"), &store, 10).expect("io"),
            Some(Capture::File { binary: true, .. })
        ));
        assert_eq!(capture_file(&d, &store, 10).expect("io"), None);
        assert!(
            !same_capture(&Capture::Oversized, &Capture::Oversized),
            "an unread side is never known to match"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
