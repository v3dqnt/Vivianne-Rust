//! Turning the attachments of a conversation into what a model request carries. A saved message holds only
//! references; here, at request time, each image becomes a picture sized for the route and each file becomes one
//! line naming its read-only copy. Nothing made here is kept: the next request makes it again from the store, and the
//! conversation never holds a path or any bytes.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vivianne_attachments::image::request::Target;
use vivianne_attachments::{Cancel, Store};
use vivianne_core::Message;
use vivianne_core::attachment::{AttachmentRef, FileRef, ImageRef};
use vivianne_providers::ImagePart;

/// Picture size and byte budget a route takes. Providers differ; this is a safe size for each family.
#[must_use]
pub fn route_target(provider: &str) -> Target {
    match provider {
        // Anthropic resizes anything above about 1568 px on the long edge and takes at most 5 MB after base64.
        "anthropic" | "claude-acp" => Target { width: 1568, height: 1568, max_bytes: 3_750_000 },
        _ => Target { width: 2048, height: 2048, max_bytes: 4 * 1024 * 1024 },
    }
}

/// The first twelve digits of an attachment's digest: enough to tell two apart in a line of text.
fn short(id: &vivianne_core::attachment::AttachmentId) -> &str {
    id.digest().get(..12).unwrap_or(id.digest())
}

/// The one line a model gets for a file: its name, size, digest and the read-only path to read it with file tools.
#[must_use]
pub fn file_line(store: &Store, file: &FileRef) -> String {
    match store.file_host_path(file) {
        Ok(path) => format!(
            "Attached file `{}` ({} bytes, sha256:{}). Read-only copy, read it with your file tools when needed: {}",
            file.name,
            file.bytes,
            short(&file.attachment_id),
            path.display()
        ),
        Err(e) => format!("Attached file `{}` could not be located ({}).", file.name, e.code().map_or("cancelled", |c| c.as_str())),
    }
}

fn image_label(store: &Store, image: &ImageRef) -> String {
    let name = image.name.as_deref().unwrap_or("image");
    let at = store.image_host_path(image).map(|p| format!(" Read-only file: {}", p.display())).unwrap_or_default();
    format!("Attached image `{name}` (id sha256:{}, {}x{}).{at}", short(&image.attachment_id), image.width, image.height)
}

/// What one message adds to a request beyond its text: lines of text, and pictures.
#[derive(Debug, Default)]
pub struct Extras {
    pub text: Vec<String>,
    pub images: Vec<(String, String, String)>,
}

/// Resolves the attachments of one message for `provider`. An image that cannot be read is named in the text with
/// the reason, so the model and the person both see it; it does not fail the whole conversation.
pub fn extras_of(store: &Store, message: &Message, provider: &str, cancel: &Cancel) -> Extras {
    let target = route_target(provider);
    let mut out = Extras::default();
    for attachment in &message.attachments {
        match attachment {
            AttachmentRef::File(file) => out.text.push(file_line(store, file)),
            AttachmentRef::Image(image) => match store.read_image_request(image, &target, cancel) {
                Ok(version) => out.images.push((version.media_type.mime().to_owned(), version.to_base64(), image_label(store, image))),
                Err(e) => {
                    let why = e.code().map_or("cancelled", |c| c.as_str());
                    vivianne_core::debuglog::log("attachments", format!("image {} not used: {why}", short(&image.attachment_id)));
                    out.text.push(format!("(Attached image `{}` could not be used: {why}.)", image.name.as_deref().unwrap_or("image")));
                }
            },
        }
    }
    out
}

/// A conversation as a request carries it.
#[derive(Debug, Default)]
pub struct Projection {
    pub messages: Vec<Message>,
    pub images: Vec<ImagePart>,
}

fn project_blocking(store: &Store, history: &[Message], provider: &str, cancel: &Cancel) -> Projection {
    let mut out = Projection { messages: Vec::with_capacity(history.len()), images: Vec::new() };
    for (index, message) in history.iter().enumerate() {
        if message.attachments.is_empty() {
            out.messages.push(message.clone());
            continue;
        }
        let extras = extras_of(store, message, provider, cancel);
        let mut text = message.text.clone();
        for line in &extras.text {
            text.push_str("\n\n");
            text.push_str(line);
        }
        out.messages.push(Message { text, attachments: Vec::new(), ..message.clone() });
        out.images.extend(extras.images.into_iter().map(|(media_type, base64, label)| ImagePart { message: index, media_type, base64, label }));
    }
    out
}

/// Projects `history` for `provider`. Reading and resizing images is slow, so it runs off the async executor and stops
/// when `cancel` fires. A history with no attachments is returned as it is, at no cost.
pub async fn project(store: &Arc<Store>, history: &[Message], provider: &str, cancel: &CancellationToken) -> Projection {
    if history.iter().all(|m| m.attachments.is_empty()) {
        return Projection { messages: history.to_vec(), images: Vec::new() };
    }
    let (store, history, provider) = (store.clone(), history.to_vec(), provider.to_owned());
    let stop = Cancel::new();
    let watcher = {
        let (stop, token) = (stop.clone(), cancel.clone());
        tokio::spawn(async move {
            token.cancelled().await;
            stop.cancel();
        })
    };
    let work = tokio::task::spawn_blocking(move || project_blocking(&store, &history, &provider, &stop));
    let outcome = work.await;
    watcher.abort();
    outcome.unwrap_or_default()
}
