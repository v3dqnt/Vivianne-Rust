//! Durable references to attached images and files. A reference is what a saved thread keeps: an id, a name and
//! facts about the stored bytes. It never holds a path, a URL or base64, so a thread file carries nothing a person
//! would not want shared. The bytes live in the attachment store (`crates/attachments`).

use serde::{Deserialize, Serialize};

const ID_PREFIX: &str = "sha256:";

/// Opaque content address of one stored object: `sha256:` and 64 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttachmentId(String);

impl AttachmentId {
    /// The id of bytes whose SHA-256 digest is `hex`, or `None` when `hex` is not 64 lowercase hex digits.
    #[must_use]
    pub fn from_digest(hex: &str) -> Option<Self> {
        is_digest(hex).then(|| Self(format!("{ID_PREFIX}{hex}")))
    }

    /// Reads an id from text, checking its shape. A malformed id is refused, never repaired.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::from_digest(text.strip_prefix(ID_PREFIX)?)
    }

    /// The digest in hex, without the prefix.
    #[must_use]
    pub fn digest(&self) -> &str {
        self.0.strip_prefix(ID_PREFIX).unwrap_or_default()
    }

    /// The whole id, `sha256:...`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_digest(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Raster formats accepted as images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "image/gif")]
    Gif,
}

impl MediaType {
    pub const ALL: [Self; 4] = [Self::Png, Self::Jpeg, Self::Webp, Self::Gif];

    /// The MIME type, `image/png` and so on.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }

    /// The file extension, without a dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
        }
    }

    /// From a MIME type, as sent in a declared upload.
    #[must_use]
    pub fn from_mime(mime: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.mime() == mime)
    }
}

/// Width and height in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

/// One stored, normalized image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub attachment_id: AttachmentId,
    pub media_type: MediaType,
    /// Exact encoded length.
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// Display name with every folder removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The picture's size before normalization shrank it. Present only when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_dimensions: Option<Dimensions>,
}

/// One file stored byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub attachment_id: AttachmentId,
    /// Safe display name, also the stored object's file name.
    pub name: String,
    pub bytes: u64,
}

/// What a message carries beside its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AttachmentRef {
    Image(ImageRef),
    File(FileRef),
}

impl AttachmentRef {
    /// The name to show for it.
    #[must_use]
    pub fn display_name(&self) -> &str {
        match self {
            Self::Image(i) => i.name.as_deref().unwrap_or("image"),
            Self::File(f) => &f.name,
        }
    }

    #[must_use]
    pub fn id(&self) -> &AttachmentId {
        match self {
            Self::Image(i) => &i.attachment_id,
            Self::File(f) => &f.attachment_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn an_id_is_sha256_and_64_lowercase_hex_digits_and_nothing_else() {
        let id = AttachmentId::parse(&format!("sha256:{HEX}")).expect("valid");
        assert_eq!(id.digest(), HEX);
        for bad in [
            "",
            "sha256:",
            &format!("sha256:{}", HEX.to_uppercase()),
            &format!("sha256:{HEX}0"),
            &format!("sha1:{HEX}"),
            &format!("sha256:{}/", &HEX[..63]),
            &format!("sha256:../{}", &HEX[3..]),
        ] {
            assert!(AttachmentId::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_reference_round_trips_and_keeps_no_path() {
        let image = AttachmentRef::Image(ImageRef {
            attachment_id: AttachmentId::from_digest(HEX).expect("valid"),
            media_type: MediaType::Png,
            bytes: 10,
            width: 4,
            height: 3,
            name: Some("shot.png".into()),
            original_dimensions: None,
        });
        let json = serde_json::to_string(&image).expect("serializes");
        assert!(json.contains(r#""type":"image""#) && json.contains("image/png"));
        assert!(!json.contains("originalDimensions"));
        assert_eq!(
            serde_json::from_str::<AttachmentRef>(&json).expect("reads"),
            image
        );
        // A damaged id still reads (a thread must open); the store refuses it when asked for the bytes.
        let damaged = serde_json::from_str::<AttachmentRef>(
            r#"{"type":"file","attachmentId":"nope","name":"a","bytes":1}"#,
        );
        assert!(damaged.is_ok_and(|r| AttachmentId::parse(r.id().as_str()).is_none()));
    }

    #[test]
    fn media_types_are_found_by_mime() {
        assert_eq!(MediaType::from_mime("image/webp"), Some(MediaType::Webp));
        assert_eq!(MediaType::from_mime("image/bmp"), None);
    }
}
