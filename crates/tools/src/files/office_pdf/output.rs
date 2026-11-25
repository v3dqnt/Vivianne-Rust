//! Reads only the bounded, regular PDF an engine made inside a private task folder.

use std::io::Read as _;
use std::path::Path;

use super::types::{Code, Error};

/// Bytes at the end of the file searched for the end-of-file marker.
const TAIL_BYTES: usize = 1024;

/// Refuses a missing, link-shaped, oversized, truncated or non-PDF output. `limit` is the inclusive PDF byte limit.
/// The bytes are independent of the scratch file.
pub fn read_pdf(path: &Path, limit: usize) -> Result<Vec<u8>, Error> {
    let not_regular = || {
        Error::new(
            Code::InvalidOutput,
            "The converter output is not a regular file.",
        )
    };
    // A link is refused before it is opened, so it cannot point the read at some other file.
    let entry = std::fs::symlink_metadata(path).map_err(|e| {
        Error::new(Code::InvalidOutput, "The converter PDF could not be read.")
            .with_detail(e.to_string())
    })?;
    if !entry.file_type().is_file() {
        return Err(not_regular());
    }
    let file = std::fs::File::open(path).map_err(|e| {
        Error::new(Code::InvalidOutput, "The converter PDF could not be read.")
            .with_detail(e.to_string())
    })?;
    let info = file.metadata().map_err(|e| {
        Error::new(Code::InvalidOutput, "The converter PDF could not be read.")
            .with_detail(e.to_string())
    })?;
    if !info.is_file() {
        return Err(not_regular());
    }
    let size = usize::try_from(info.len()).unwrap_or(usize::MAX);
    if size > limit {
        return Err(Error::new(
            Code::OutputTooLarge,
            "The converted PDF exceeds maxOutputBytes.",
        ));
    }
    // One byte past the size it reported: a file that grew while it was read is caught.
    let mut bytes = Vec::with_capacity(size + 1);
    file.take(size as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            Error::new(Code::InvalidOutput, "The converter PDF could not be read.")
                .with_detail(e.to_string())
        })?;
    if bytes.len() > limit {
        return Err(Error::new(
            Code::OutputTooLarge,
            "The converted PDF exceeds maxOutputBytes.",
        ));
    }
    let tail = &bytes[bytes.len().saturating_sub(TAIL_BYTES)..];
    let tail = String::from_utf8_lossy(tail);
    if bytes.len() != size || !has_pdf_header(&bytes) || !tail.trim_end().ends_with("%%EOF") {
        return Err(Error::new(
            Code::InvalidOutput,
            "The converter did not produce a complete PDF.",
        ));
    }
    Ok(bytes)
}

/// `%PDF-d.d` at the start.
fn has_pdf_header(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && &bytes[..5] == b"%PDF-"
        && bytes[5].is_ascii_digit()
        && bytes[6] == b'.'
        && bytes[7].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        crate::testing::scratch("pdf-out")
    }

    const GOOD: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\ntrailer\n%%EOF\n";

    #[test]
    fn a_complete_pdf_is_read_whole() {
        let d = dir();
        std::fs::write(d.join("a.pdf"), GOOD).expect("write");
        assert_eq!(read_pdf(&d.join("a.pdf"), 1000).expect("reads"), GOOD);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_oversized_truncated_and_non_pdf_output_is_refused_with_its_class() {
        let d = dir();
        assert_eq!(
            read_pdf(&d.join("none.pdf"), 1000).unwrap_err().code,
            Code::InvalidOutput
        );
        std::fs::write(d.join("big.pdf"), GOOD).expect("write");
        assert_eq!(
            read_pdf(&d.join("big.pdf"), 10).unwrap_err().code,
            Code::OutputTooLarge
        );
        std::fs::write(d.join("cut.pdf"), &GOOD[..GOOD.len() - 8]).expect("write");
        assert_eq!(
            read_pdf(&d.join("cut.pdf"), 1000).unwrap_err().code,
            Code::InvalidOutput
        );
        std::fs::write(d.join("text.pdf"), "hello %%EOF").expect("write");
        assert_eq!(
            read_pdf(&d.join("text.pdf"), 1000).unwrap_err().code,
            Code::InvalidOutput
        );
        std::fs::write(d.join("v.pdf"), "%PDF-x.y\n%%EOF").expect("write");
        assert_eq!(
            read_pdf(&d.join("v.pdf"), 1000).unwrap_err().code,
            Code::InvalidOutput
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_is_not_a_pdf() {
        let d = dir();
        assert_eq!(read_pdf(&d, 1000).unwrap_err().code, Code::InvalidOutput);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_refused_even_when_it_points_at_a_pdf() {
        let d = dir();
        std::fs::write(d.join("real.pdf"), GOOD).expect("write");
        std::os::unix::fs::symlink(d.join("real.pdf"), d.join("link.pdf")).expect("link");
        assert_eq!(
            read_pdf(&d.join("link.pdf"), 1000).unwrap_err().code,
            Code::InvalidOutput
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trailing_whitespace_after_the_marker_is_fine() {
        let d = dir();
        std::fs::write(d.join("a.pdf"), b"%PDF-1.4\nbody\n%%EOF \r\n\n").expect("write");
        assert!(read_pdf(&d.join("a.pdf"), 1000).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }
}
