//! Request versions: sized for a route, cached, shared between concurrent callers, rebuilt when the cache is cleared.

mod common;

use std::sync::Arc;

use common::{Scratch, dimensions_of, gradient, noise, png, png_rgba};
use image::{Rgba, RgbaImage};
use vivianne_attachments::image::request::{Target, variant_id};
use vivianne_attachments::{Cancel, Code, Config, Error, SaveImage, Store};
use vivianne_core::attachment::{ImageRef, MediaType};

fn save(store: &Store, data: Vec<u8>) -> ImageRef {
    store.save_image(&SaveImage { data, media_type: MediaType::Png, name: None }).expect("saved")
}

fn target(width: u32, height: u32, max_bytes: u64) -> Target {
    Target { width, height, max_bytes }
}

fn store_in(home: &Scratch) -> Arc<Store> {
    Arc::new(home.store(Config::default()))
}

#[test]
fn an_in_budget_image_passes_through_and_a_larger_target_keeps_the_source() {
    let home = Scratch::new("req-pass");
    let store = store_in(&home);
    let data = png(&gradient(80, 60));
    let image = save(&store, data.clone());
    let version = store.read_image_request(&image, &target(1000, 1000, 1 << 20), &Cancel::new()).expect("version");
    assert_eq!(version.data, data);
    assert_eq!((version.width, version.height), (80, 60));
    assert!(!store.layout().cache.exists(), "a pass-through writes no cache file");
}

#[test]
fn the_long_edge_is_resized_to_the_exact_target_and_the_short_edge_rounds() {
    let home = Scratch::new("req-resize");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(1000, 700)));
    let version = store.read_image_request(&image, &target(300, 300, 1 << 20), &Cancel::new()).expect("version");
    assert_eq!((version.width, version.height), (300, 210));
    assert_eq!(dimensions_of(&version.data), (300, 210));
    let tall = save(&store, png(&gradient(700, 1000)));
    let version = store.read_image_request(&tall, &target(300, 300, 1 << 20), &Cancel::new()).expect("version");
    assert_eq!((version.width, version.height), (210, 300));
}

#[test]
fn the_cache_key_covers_every_input_and_a_cleared_cache_is_rebuilt_without_loss() {
    let home = Scratch::new("req-cache");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(900, 600)));
    let t = target(300, 300, 1 << 20);
    let first = store.read_image_request(&image, &t, &Cancel::new()).expect("first");
    let other = store.read_image_request(&image, &target(200, 200, 1 << 20), &Cancel::new()).expect("other");
    assert_ne!(first.variant_id, other.variant_id);
    assert_ne!(variant_id(&image.attachment_id, &t), variant_id(&image.attachment_id, &target(300, 300, 1 << 19)));
    assert_eq!(first.variant_id, variant_id(&image.attachment_id, &t));
    let cache = store.layout().cache.clone();
    assert!(cache.join("request-images").exists());

    let again = store.read_image_request(&image, &t, &Cancel::new()).expect("cached");
    assert_eq!(again.data, first.data, "the same bytes come back from the cache");

    std::fs::remove_dir_all(&cache).expect("cleared");
    let rebuilt = store.read_image_request(&image, &t, &Cancel::new()).expect("rebuilt");
    assert_eq!(rebuilt.data, first.data, "deterministic: the rebuilt version is the same");
    assert!(store.read_image(&image, &Cancel::new()).is_ok(), "durable attachments are untouched");
}

#[test]
fn a_damaged_cached_version_is_regenerated() {
    let home = Scratch::new("req-damaged");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(900, 600)));
    let t = target(300, 300, 1 << 20);
    let good = store.read_image_request(&image, &t, &Cancel::new()).expect("first");
    let hash = good.variant_id.trim_start_matches("sha256:").to_owned();
    let path = store.layout().request_image_path(&hash);
    std::fs::write(&path, b"not an image").expect("damage");
    assert_eq!(store.read_image_request(&image, &t, &Cancel::new()).expect("rebuilt").data, good.data);
    // A cached image larger than the target is refused too.
    std::fs::write(&path, png(&gradient(500, 400))).expect("too big");
    assert_eq!(store.read_image_request(&image, &t, &Cancel::new()).expect("rebuilt").data, good.data);
}

#[test]
fn an_unreachable_byte_target_keeps_the_smallest_ladder_output() {
    let home = Scratch::new("req-bytes");
    let store = store_in(&home);
    let image = save(&store, png(&noise(400, 400)));
    let version = store.read_image_request(&image, &target(200, 200, 1), &Cancel::new()).expect("version");
    assert!(version.data.len() > 1);
    assert_eq!((version.width, version.height), (200, 200));
}

#[test]
fn opaque_pixels_go_to_jpeg_and_transparency_to_the_webp_ladder() {
    let home = Scratch::new("req-codec");
    let store = store_in(&home);
    let opaque = save(&store, png(&gradient(600, 400)));
    assert_eq!(store.read_image_request(&opaque, &target(200, 200, 1 << 20), &Cancel::new()).expect("v").media_type, MediaType::Jpeg);
    let mut img = RgbaImage::from_pixel(600, 400, Rgba([10, 200, 30, 255]));
    img.put_pixel(3, 3, Rgba([0, 0, 0, 0]));
    let clear = store.save_image(&SaveImage { data: png_rgba(&img), media_type: MediaType::Png, name: None }).expect("saved");
    let version = store.read_image_request(&clear, &target(200, 200, 1 << 20), &Cancel::new()).expect("v");
    assert_eq!(version.media_type, MediaType::Webp);
}

#[test]
fn a_target_with_a_zero_side_or_budget_is_refused() {
    let home = Scratch::new("req-invalid");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(20, 20)));
    for bad in [target(0, 10, 10), target(10, 0, 10), target(10, 10, 0)] {
        let outcome = store.read_image_request(&image, &bad, &Cancel::new());
        assert_eq!(outcome.err().and_then(|e| e.code()), Some(Code::InvalidAttachmentRef));
    }
}

#[test]
fn concurrent_callers_share_one_transformation_without_sharing_cancellation() {
    let home = Scratch::new("req-shared");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(1200, 800)));
    let t = target(300, 300, 1 << 20);
    let patient = Cancel::new();
    let impatient = Cancel::new();
    impatient.cancel();
    let results: Vec<_> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| store.read_image_request(&image, &t, &patient)))
            .collect();
        let early = scope.spawn(|| store.read_image_request(&image, &t, &impatient));
        let mut all: Vec<_> = workers.into_iter().map(|w| w.join().expect("joined")).collect();
        all.push(early.join().expect("joined"));
        all
    });
    assert_eq!(results.last().expect("early"), &Err(Error::Cancelled));
    let versions: Vec<_> = results[..4].iter().map(|r| r.as_ref().expect("version")).collect();
    assert!(versions.iter().all(|v| v.data == versions[0].data));
}

#[test]
fn a_request_cancelled_by_its_only_waiter_stops_and_a_later_request_succeeds() {
    let home = Scratch::new("req-cancelled");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(1200, 800)));
    let t = target(300, 300, 1 << 20);
    let cancel = Cancel::new();
    cancel.cancel();
    assert_eq!(store.read_image_request(&image, &t, &cancel), Err(Error::Cancelled));
    assert!(store.read_image_request(&image, &t, &Cancel::new()).is_ok());
}

#[test]
fn a_missing_stored_image_fails_the_request_with_not_found() {
    let home = Scratch::new("req-missing");
    let store = store_in(&home);
    let image = save(&store, png(&gradient(900, 600)));
    let path = store.image_host_path(&image).expect("path");
    vivianne_attachments::objects::make_writable(&path).expect("writable");
    std::fs::remove_file(path).expect("removed");
    let outcome = store.read_image_request(&image, &target(300, 300, 1 << 20), &Cancel::new());
    assert_eq!(outcome.err().and_then(|e| e.code()), Some(Code::AttachmentNotFound));
}
