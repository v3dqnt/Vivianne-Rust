//! Admitting a whole prompt: text, encoded images, and references to things already stored. Every image counts
//! toward the per-message image and byte limits; refused content starts no writes.

use vivianne_core::attachment::{FileRef, ImageRef, MediaType};

use crate::admission::{Empty, decode_canonical_base64};
use crate::error::{Code, Result, fail};
use crate::prepare::SaveImage;
use crate::store::Store;

/// One part of a prompt as it arrives.
#[derive(Debug, Clone)]
pub enum PromptPart {
    Text(String),
    /// An image as base64, with the type its sender declared.
    EncodedImage {
        media_type: String,
        data: String,
        name: Option<String>,
    },
    /// An image an earlier upload of the same session stored; `encoded_bytes` is that upload's length.
    StoredImage {
        image: ImageRef,
        encoded_bytes: u64,
    },
    File(FileRef),
}

/// A prompt part with every attachment as a durable reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    Text(String),
    Image(ImageRef),
    File(FileRef),
}

/// The decoded length of canonical base64, without decoding it.
fn decoded_len(data: &str) -> u64 {
    let padding = data.bytes().rev().take_while(|b| *b == b'=').count().min(2);
    (data.len() as u64 / 4) * 3 - padding as u64
}

impl Store {
    /// Admits encoded images: canonical base64 for every member, then one ordered batch.
    pub fn admit_encoded_images(
        &self,
        images: &[(String, String, Option<String>)],
    ) -> Result<Vec<ImageRef>> {
        let mut inputs = Vec::with_capacity(images.len());
        for (mime, data, name) in images {
            let Some(media_type) = MediaType::from_mime(mime) else {
                return fail(
                    Code::UnsupportedImageType,
                    format!("Image type {mime} is not accepted by this deployment."),
                );
            };
            inputs.push(SaveImage {
                data: decode_canonical_base64(data, Empty::Reject, Code::InvalidImageBase64)?,
                media_type,
                name: name.clone(),
            });
        }
        self.save_images(&inputs)
    }

    /// Admits one encoded file: canonical base64 (an empty file is a valid zero-byte upload), then a verbatim save.
    pub fn admit_encoded_file(&self, data: &str, name: Option<&str>) -> Result<FileRef> {
        let bytes = decode_canonical_base64(data, Empty::Accept, Code::InvalidFileBase64)?;
        self.save_file(&bytes, name)
    }

    /// Admits a prompt in part order. Text, files and stored images pass through; encoded images are stored.
    pub fn admit_prompt_content(&self, content: &[PromptPart]) -> Result<Vec<Admitted>> {
        let encoded: Vec<_> = content
            .iter()
            .filter_map(|p| match p {
                PromptPart::EncodedImage {
                    media_type,
                    data,
                    name,
                } => Some((media_type.clone(), data.clone(), name.clone())),
                _ => None,
            })
            .collect();
        let stored: Vec<u64> = content
            .iter()
            .filter_map(|p| {
                if let PromptPart::StoredImage { encoded_bytes, .. } = p {
                    Some(*encoded_bytes)
                } else {
                    None
                }
            })
            .collect();
        // Each upload passed the limits alone; the encoded batch checks only itself, so the whole message is checked here.
        if !stored.is_empty() {
            let bytes = encoded
                .iter()
                .map(|(_, data, _)| decoded_len(data))
                .sum::<u64>()
                + stored.iter().sum::<u64>();
            self.check_batch(encoded.len() + stored.len(), bytes, [])?;
        }
        let refs = if encoded.is_empty() {
            Vec::new()
        } else {
            self.admit_encoded_images(&encoded)?
        };
        let mut next = refs.into_iter();
        content
            .iter()
            .map(|part| match part {
                PromptPart::Text(text) => Ok(Admitted::Text(text.clone())),
                PromptPart::File(file) => Ok(Admitted::File(file.clone())),
                PromptPart::StoredImage { image, .. } => Ok(Admitted::Image(image.clone())),
                PromptPart::EncodedImage { .. } => next.next().map(Admitted::Image).map_or_else(
                    || fail(Code::AttachmentWriteFailed, "An admitted image is missing."),
                    Ok,
                ),
            })
            .collect()
    }
}
