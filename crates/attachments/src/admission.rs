//! Wire-form uploads: base64 text from a client. Only the canonical encoding is accepted, so one set of bytes has
//! one spelling and nothing hides in padding or whitespace.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::error::{Code, Result, fail};

/// Whether an empty payload is a valid upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Empty {
    Reject,
    Accept,
}

/// Decodes one upload, refusing every non-canonical form: other alphabets, missing or extra padding, whitespace,
/// line breaks, and trailing bits that are not zero.
pub fn decode_canonical_base64(data: &str, empty: Empty, code: Code) -> Result<Vec<u8>> {
    let what = if code == Code::InvalidFileBase64 {
        "File"
    } else {
        "Image"
    };
    let bad = || fail(code, format!("{what} upload is not canonical base64."));
    if data.is_empty() {
        return if empty == Empty::Accept {
            Ok(Vec::new())
        } else {
            bad()
        };
    }
    let Ok(decoded) = STANDARD.decode(data) else {
        return bad();
    };
    if STANDARD.encode(&decoded) != data {
        return bad();
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(data: &str) -> Result<Vec<u8>> {
        decode_canonical_base64(data, Empty::Reject, Code::InvalidImageBase64)
    }

    #[test]
    fn canonical_base64_decodes() {
        assert_eq!(image("aGk=").expect("valid"), b"hi");
        assert_eq!(image("aGVsbG8=").expect("valid"), b"hello");
    }

    #[test]
    fn every_non_canonical_spelling_is_refused() {
        for bad in [
            "aGk",
            "aGk==",
            "aG k=",
            "aGk=\n",
            "aGl=",
            "aGk=aGk=",
            "a",
            "!!!!",
            "aGk\u{0}=",
            "-_-_",
        ] {
            assert_eq!(
                image(bad).map_err(|e| e.code()),
                Err(Some(Code::InvalidImageBase64)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_empty_image_is_refused_but_an_empty_file_is_a_zero_byte_file() {
        assert!(image("").is_err());
        let file = decode_canonical_base64("", Empty::Accept, Code::InvalidFileBase64);
        assert_eq!(file.expect("accepted"), Vec::<u8>::new());
    }

    #[test]
    fn a_file_failure_carries_the_file_code() {
        let e = decode_canonical_base64("zz", Empty::Accept, Code::InvalidFileBase64)
            .expect_err("refused");
        assert_eq!(e.code(), Some(Code::InvalidFileBase64));
    }
}
