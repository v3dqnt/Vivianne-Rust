//! Admitting a whole prompt: ordering, limits that count stored uploads, and canonical base64.

mod common;

use base64::Engine as _;
use common::{Scratch, gif, gradient, png};
use vivianne_attachments::{Admitted, Code, Config, Limits, PromptPart};
use vivianne_core::attachment::MediaType;

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn image_part(data: &[u8]) -> PromptPart {
    PromptPart::EncodedImage { media_type: "image/png".into(), data: b64(data), name: Some("a.png".into()) }
}

fn code<T>(r: Result<T, vivianne_attachments::Error>) -> Option<Code> {
    r.err().and_then(|e| e.code())
}

#[test]
fn text_and_files_pass_through_without_touching_image_storage() {
    let home = Scratch::new("prompt-text");
    let store = home.store(Config::default());
    let file = store.save_file(b"x", Some("a.txt")).expect("file");
    let admitted = store.admit_prompt_content(&[PromptPart::Text("hi".into()), PromptPart::File(file.clone())]).expect("admitted");
    assert_eq!(admitted, vec![Admitted::Text("hi".into()), Admitted::File(file)]);
    assert!(!home.path().join("attachments").join("v1").join("objects").exists());
}

#[test]
fn images_are_stored_and_every_part_keeps_its_place() {
    let home = Scratch::new("prompt-order");
    let store = home.store(Config::default());
    let first = png(&gradient(20, 10));
    let second = png(&gradient(30, 10));
    let admitted = store
        .admit_prompt_content(&[PromptPart::Text("look".into()), image_part(&first), PromptPart::Text("and".into()), image_part(&second)])
        .expect("admitted");
    let widths: Vec<u32> = admitted.iter().filter_map(|a| if let Admitted::Image(i) = a { Some(i.width) } else { None }).collect();
    assert_eq!(widths, vec![20, 30]);
    assert!(matches!(admitted[0], Admitted::Text(_)) && matches!(admitted[2], Admitted::Text(_)));
}

#[test]
fn stored_images_pass_through_and_count_toward_the_message_limits() {
    let home = Scratch::new("prompt-stored");
    let limits = Limits { max_images_per_message: 2, max_message_image_bytes: 3000, ..Limits::default() };
    let store = home.store(Config { limits, ..Config::default() });
    let stored = store
        .save_image(&vivianne_attachments::SaveImage { data: png(&gradient(16, 16)), media_type: MediaType::Png, name: None })
        .expect("stored");
    let one_stored = PromptPart::StoredImage { image: stored.clone(), encoded_bytes: 1000 };
    let passed = store.admit_prompt_content(&[one_stored.clone(), PromptPart::Text("x".into())]).expect("admitted");
    assert_eq!(passed[0], Admitted::Image(stored.clone()));

    // Count: one stored + two encoded is three images.
    let img = png(&gradient(8, 8));
    let outcome = store.admit_prompt_content(&[one_stored.clone(), image_part(&img), image_part(&img)]);
    assert_eq!(code(outcome), Some(Code::TooManyImages));
    // Bytes: the stored upload's length counts with the encoded bytes.
    let heavy = PromptPart::StoredImage { image: stored, encoded_bytes: 2999 };
    let outcome = store.admit_prompt_content(&[heavy, image_part(&img)]);
    assert_eq!(code(outcome), Some(Code::ImagesTooLarge));
    assert!(!home.path().join("attachments").join("v1").join("objects").join("zz").exists());
}

#[test]
fn a_refused_prompt_starts_no_writes() {
    let home = Scratch::new("prompt-refused");
    let store = home.store(Config::default());
    let good = png(&gradient(16, 16));
    let outcome = store.admit_prompt_content(&[image_part(&good), PromptPart::EncodedImage { media_type: "image/png".into(), data: "not base64!".into(), name: None }]);
    assert_eq!(code(outcome), Some(Code::InvalidImageBase64));
    let outcome = store.admit_prompt_content(&[image_part(&good), PromptPart::EncodedImage { media_type: "image/bmp".into(), data: b64(b"x"), name: None }]);
    assert_eq!(code(outcome), Some(Code::UnsupportedImageType));
    assert!(!home.path().join("attachments").exists());
}

#[test]
fn encoded_image_and_file_uploads_must_be_canonical_base64() {
    let home = Scratch::new("prompt-b64");
    let store = home.store(Config::default());
    let good = b64(&png(&gradient(8, 8)));
    for bad in [format!("{good}\n"), format!(" {good}"), good.trim_end_matches('=').to_owned() + "=="] {
        let outcome = store.admit_encoded_images(&[("image/png".into(), bad, None)]);
        assert!(matches!(code(outcome), Some(Code::InvalidImageBase64)), "non-canonical forms are refused");
    }
    assert_eq!(code(store.admit_encoded_images(&[("image/png".into(), String::new(), None)])), Some(Code::InvalidImageBase64));
    let file = store.admit_encoded_file("", Some("empty.txt")).expect("zero-byte file");
    assert_eq!(file.bytes, 0);
    let file = store.admit_encoded_file(&b64(b"hello"), None).expect("file");
    assert_eq!((file.bytes, file.name.as_str()), (5, "file"));
    assert_eq!(code(store.admit_encoded_file("aGk", None)), Some(Code::InvalidFileBase64));
    // Encoded gifs convert like any other source.
    let converted = store.admit_encoded_images(&[("image/gif".into(), b64(&gif(10, 10, 2)), None)]).expect("converted");
    assert_eq!(converted[0].media_type, MediaType::Jpeg);
}
