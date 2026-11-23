//! Images: admission limits, normalization to the stored form, durable save and verified read.

mod common;

use common::{
    Scratch, dimensions_of, gif, gradient, jpeg, jpeg_with_orientation, noise, png, png16, png_rgba, png_with_text, webp,
};
use image::{Rgba, RgbaImage};
use vivianne_attachments::image::detect::probe;
use vivianne_attachments::image::normalize::{Policy, can_pass_through};
use vivianne_attachments::{Cancel, Code, Config, Limits, SaveImage};
use vivianne_core::attachment::MediaType;

fn input(data: Vec<u8>, media_type: MediaType) -> SaveImage {
    SaveImage { data, media_type, name: Some("shot.png".into()) }
}

fn code<T>(r: Result<T, vivianne_attachments::Error>) -> Option<Code> {
    r.err().and_then(|e| e.code())
}

#[test]
fn a_clean_in_budget_image_is_stored_byte_for_byte() {
    let home = Scratch::new("img-pass");
    let store = home.store(Config::default());
    let data = png(&gradient(64, 48));
    let image = store.save_image(&input(data.clone(), MediaType::Png)).expect("saved");
    assert_eq!((image.media_type, image.width, image.height, image.bytes), (MediaType::Png, 64, 48, data.len() as u64));
    assert_eq!(image.original_dimensions, None);
    let stored = store.read_image(&image, &Cancel::new()).expect("verified");
    assert_eq!(stored.data, data);
    let path = store.image_host_path(&image).expect("path");
    assert!(std::fs::metadata(path).expect("meta").permissions().readonly());
}

#[test]
fn metadata_is_stripped_by_re_encoding_and_the_picture_stays_upright() {
    let home = Scratch::new("img-meta");
    let store = home.store(Config::default());
    let with_text = png_with_text(&png(&gradient(40, 30)));
    assert!(probe(&with_text).expect("probe").carries_metadata);
    let image = store.save_image(&input(with_text, MediaType::Png)).expect("saved");
    assert_eq!(image.media_type, MediaType::Jpeg, "an opaque image is re-encoded as JPEG");
    let stored = store.read_image(&image, &Cancel::new()).expect("verified");
    assert!(!probe(&stored.data).expect("probe").carries_metadata);

    // EXIF orientation 6 turns a 40x20 raster into a 20x40 picture; the rotation is baked in.
    let rotated = jpeg_with_orientation(&jpeg(&gradient(40, 20), 90), 6);
    assert_eq!((probe(&rotated).expect("probe").width, probe(&rotated).expect("probe").height), (20, 40));
    let image = store.save_image(&input(rotated, MediaType::Jpeg)).expect("saved");
    assert_eq!((image.width, image.height), (20, 40));
    let stored = store.read_image(&image, &Cancel::new()).expect("verified");
    assert_eq!(dimensions_of(&stored.data), (20, 40));
    assert!(!probe(&stored.data).expect("probe").carries_metadata);
}

#[test]
fn an_oversized_image_is_shrunk_within_the_pixel_budget_and_remembers_its_size() {
    let home = Scratch::new("img-big");
    let store = home.store(Config::default());
    let image = store.save_image(&input(png(&gradient(3000, 2000)), MediaType::Png)).expect("saved");
    assert!(u64::from(image.width) * u64::from(image.height) <= 2048 * 2048);
    assert!((i64::from(image.width) * 2 - i64::from(image.height) * 3).abs() <= 3);
    let original = image.original_dimensions.expect("recorded");
    assert_eq!((original.width, original.height), (3000, 2000));
    assert_eq!(image.media_type, MediaType::Jpeg);
}

#[test]
fn the_long_edge_cap_applies_after_the_pixel_budget() {
    let home = Scratch::new("img-long");
    let config = Config { policy: Policy { max_pixels: 4_000_000, max_dimension: 1000, max_bytes: 4_000_000 }, ..Config::default() };
    let store = home.store(config);
    let image = store.save_image(&input(png(&gradient(4000, 100)), MediaType::Png)).expect("saved");
    assert_eq!((image.width, image.height), (1000, 25));
}

#[test]
fn a_gif_is_always_re_encoded_as_one_still_frame() {
    let home = Scratch::new("img-gif");
    let store = home.store(Config::default());
    let animated = gif(30, 20, 3);
    assert!(probe(&animated).expect("probe").animated);
    let image = store.save_image(&input(animated, MediaType::Gif)).expect("saved");
    assert_eq!(image.media_type, MediaType::Jpeg);
    let still = gif(30, 20, 1);
    let image = store.save_image(&input(still, MediaType::Gif)).expect("saved");
    assert_eq!(image.media_type, MediaType::Jpeg, "even a one-frame GIF is converted");
}

#[test]
fn transparency_is_kept_on_the_webp_ladder() {
    let home = Scratch::new("img-alpha");
    let store = home.store(Config::default());
    let mut img = RgbaImage::from_pixel(40, 30, Rgba([200, 30, 30, 255]));
    for x in 0..20 {
        img.put_pixel(x, 5, Rgba([0, 0, 0, 0]));
    }
    // A PNG with an alpha channel is above the pass-through rules only when something else forces a re-encode; a
    // text chunk does.
    let source = png_with_text(&png_rgba(&img));
    let image = store.save_image(&input(source, MediaType::Png)).expect("saved");
    assert_eq!(image.media_type, MediaType::Webp);
    let stored = store.read_image(&image, &Cancel::new()).expect("verified");
    let facts = probe(&stored.data).expect("probe");
    assert!(facts.has_alpha && !facts.carries_metadata);
    // A still lossless WebP that is already clean passes through unchanged.
    let clean = webp(&img);
    let kept = store.save_image(&input(clean.clone(), MediaType::Webp)).expect("saved");
    assert_eq!(store.read_image(&kept, &Cancel::new()).expect("verified").data, clean);
}

#[test]
fn a_16_bit_png_is_converted_to_8_bits() {
    let home = Scratch::new("img-16bit");
    let store = home.store(Config::default());
    let source = png16(32, 32);
    assert!(probe(&source).expect("probe").high_depth);
    let image = store.save_image(&input(source, MediaType::Png)).expect("saved");
    let stored = store.read_image(&image, &Cancel::new()).expect("verified");
    assert!(!probe(&stored.data).expect("probe").high_depth);
}

#[test]
fn an_unreachable_byte_target_keeps_the_smallest_output_instead_of_failing() {
    let home = Scratch::new("img-target");
    let config = Config { policy: Policy { max_pixels: 4_000_000, max_dimension: 8192, max_bytes: 1 }, ..Config::default() };
    let store = home.store(config);
    let image = store.save_image(&input(png(&noise(120, 120)), MediaType::Png)).expect("saved");
    assert!(image.bytes > 1);
    assert_eq!((image.width, image.height), (120, 120), "quality drops, size does not");
}

#[test]
fn pass_through_is_refused_for_gif_animation_metadata_and_oversize() {
    let policy = Policy::default();
    let clean = probe(&png(&gradient(10, 10))).expect("probe");
    assert!(can_pass_through(&clean, 1000, &policy));
    assert!(!can_pass_through(&clean, policy.max_bytes + 1, &policy));
    assert!(!can_pass_through(&probe(&gif(5, 5, 1)).expect("probe"), 100, &policy));
    assert!(!can_pass_through(&probe(&png_with_text(&png(&gradient(10, 10)))).expect("probe"), 100, &policy));
    assert!(!can_pass_through(&probe(&png16(8, 8)).expect("probe"), 100, &policy));
    let wide = vivianne_attachments::image::detect::Detected { width: 9000, height: 10, ..clean };
    assert!(!can_pass_through(&wide, 100, &policy));
    let many = vivianne_attachments::image::detect::Detected { width: 3000, height: 3000, ..clean };
    assert!(!can_pass_through(&many, 100, &policy));
}

#[test]
fn every_supported_format_decodes_with_its_dimensions() {
    let img = gradient(17, 9);
    let rgba = RgbaImage::from_pixel(17, 9, Rgba([1, 2, 3, 255]));
    for (data, media_type) in [
        (png(&img), MediaType::Png),
        (jpeg(&img, 80), MediaType::Jpeg),
        (webp(&rgba), MediaType::Webp),
        (gif(17, 9, 1), MediaType::Gif),
    ] {
        let facts = probe(&data).expect("probe");
        assert_eq!((facts.media_type, facts.width, facts.height), (media_type, 17, 9));
    }
}

#[test]
fn size_limits_are_checked_from_the_header_and_a_side_exactly_at_the_limit_is_accepted() {
    let home = Scratch::new("img-limits");
    let limits = Limits { max_image_pixels: 1_000_000, max_image_dimension: 10, ..Limits::default() };
    let store = home.store(Config { limits, ..Config::default() });
    assert!(store.validate_image(&input(png(&gradient(10, 10)), MediaType::Png)).is_ok());
    assert_eq!(code(store.validate_image(&input(png(&gradient(11, 5)), MediaType::Png))), Some(Code::ImageDimensionTooLarge));
    assert_eq!(code(store.validate_image(&input(png(&gradient(9, 12)), MediaType::Png))), Some(Code::ImageDimensionTooLarge));
    let pixels = Limits { max_image_pixels: 100, max_image_dimension: 100, ..Limits::default() };
    let store = home.store(Config { limits: pixels, ..Config::default() });
    assert_eq!(code(store.validate_image(&input(png(&gradient(11, 10)), MediaType::Png))), Some(Code::ImageTooManyPixels));
}

#[test]
fn bad_input_is_refused_with_a_stable_code() {
    let home = Scratch::new("img-bad");
    let store = home.store(Config { limits: Limits { max_image_bytes: 5000, ..Limits::default() }, ..Config::default() });
    let good = png(&gradient(20, 20));
    assert_eq!(code(store.validate_image(&input(good.clone(), MediaType::Jpeg))), Some(Code::ImageTypeMismatch));
    assert_eq!(code(store.validate_image(&input(Vec::new(), MediaType::Png))), Some(Code::InvalidImage));
    assert_eq!(code(store.validate_image(&input(b"BM not an image at all".to_vec(), MediaType::Png))), Some(Code::InvalidImage));
    assert_eq!(code(store.validate_image(&input(good[..good.len() / 2].to_vec(), MediaType::Png))), Some(Code::InvalidImage));
    let mut broken = good.clone();
    let mid = broken.len() / 2;
    broken[mid..mid + 40].fill(0xA5);
    assert!(store.validate_image(&input(broken, MediaType::Png)).is_err());
    let big = png(&noise(100, 100));
    assert!(big.len() > 5000);
    assert_eq!(code(store.validate_image(&input(big, MediaType::Png))), Some(Code::ImageTooLarge));
}

#[test]
fn a_batch_is_checked_whole_before_any_member_is_written() {
    let home = Scratch::new("img-batch");
    let limits = Limits {
        max_images_per_message: 2,
        max_message_image_bytes: 100_000,
        media_types: vec![MediaType::Png, MediaType::Jpeg],
        ..Limits::default()
    };
    let store = home.store(Config { limits, ..Config::default() });
    let a = input(png(&gradient(16, 16)), MediaType::Png);
    let b = input(png(&gradient(18, 18)), MediaType::Png);
    assert_eq!(code(store.save_images(&[a.clone(), b.clone(), a.clone()])), Some(Code::TooManyImages));
    let g = input(gif(8, 8, 1), MediaType::Gif);
    assert_eq!(code(store.save_images(&[a.clone(), g])), Some(Code::UnsupportedImageType));
    let huge = input(vec![0u8; 100_001], MediaType::Png);
    assert_eq!(code(store.save_images(&[huge])), Some(Code::ImagesTooLarge));
    // A bad member after a good one: the good one is not written either.
    let bad = input(b"nope".to_vec(), MediaType::Png);
    assert_eq!(code(store.save_images(&[a.clone(), bad])), Some(Code::InvalidImage));
    assert!(!home.path().join("attachments").exists(), "nothing was written");
    // A good batch comes back in input order.
    let refs = store.save_images(&[b.clone(), a.clone()]).expect("saved");
    assert_eq!((refs[0].width, refs[1].width), (18, 16));
}

#[test]
fn equal_images_share_one_object_and_validation_stores_nothing() {
    let home = Scratch::new("img-dedupe");
    let store = home.store(Config::default());
    let data = png(&gradient(30, 30));
    assert!(store.validate_image(&input(data.clone(), MediaType::Png)).is_ok());
    assert!(!home.path().join("attachments").exists());
    let a = store.save_image(&input(data.clone(), MediaType::Png)).expect("a");
    let b = store.save_image(&SaveImage { name: Some("other.png".into()), ..input(data, MediaType::Png) }).expect("b");
    assert_eq!(a.attachment_id, b.attachment_id);
    assert_eq!(b.name.as_deref(), Some("other.png"));
}

#[test]
fn a_name_from_another_machine_is_cut_to_its_last_part() {
    let home = Scratch::new("img-name");
    let store = home.store(Config::default());
    let image = store.save_image(&SaveImage { name: Some("C:\\Users\\ana\\Pictures\\cat.png".into()), ..input(png(&gradient(8, 8)), MediaType::Png) }).expect("saved");
    assert_eq!(image.name.as_deref(), Some("cat.png"));
    let json = serde_json::to_string(&image).expect("json");
    assert!(!json.contains("Users") && !json.contains("base64"));
}

#[test]
fn reads_fail_closed_when_an_object_is_missing_damaged_or_swapped() {
    let home = Scratch::new("img-read");
    let store = home.store(Config::default());
    let image = store.save_image(&input(png(&gradient(24, 24)), MediaType::Png)).expect("saved");
    let cancel = Cancel::new();

    let mut wrong_size = image.clone();
    wrong_size.width += 1;
    assert_eq!(code(store.read_image(&wrong_size, &cancel)), Some(Code::AttachmentCorrupt));
    let mut wrong_type = image.clone();
    wrong_type.media_type = MediaType::Jpeg;
    assert_eq!(code(store.read_image(&wrong_type, &cancel)), Some(Code::AttachmentCorrupt));
    let mut wrong_length = image.clone();
    wrong_length.bytes += 1;
    assert_eq!(code(store.read_image(&wrong_length, &cancel)), Some(Code::AttachmentCorrupt));
    let mut bad_id = image.clone();
    bad_id.attachment_id = serde_json::from_str("\"sha256:zz\"").expect("reads");
    assert_eq!(code(store.read_image(&bad_id, &cancel)), Some(Code::InvalidAttachmentRef));

    let path = store.image_host_path(&image).expect("path");
    vivianne_attachments::objects::make_writable(&path).expect("writable");
    let original = std::fs::read(&path).expect("bytes");
    std::fs::write(&path, b"swapped").expect("swap");
    assert_eq!(code(store.read_image(&image, &cancel)), Some(Code::AttachmentCorrupt));
    std::fs::remove_file(&path).expect("removed");
    assert_eq!(code(store.read_image(&image, &cancel)), Some(Code::AttachmentNotFound));
    std::fs::write(&path, original).expect("restored");
    assert!(store.read_image(&image, &cancel).is_ok());

    let cancelled = Cancel::new();
    cancelled.cancel();
    assert_eq!(store.read_image(&image, &cancelled).map(|_| ()), Err(vivianne_attachments::Error::Cancelled));
}

#[test]
fn history_stays_readable_after_the_limits_become_stricter() {
    let home = Scratch::new("img-history");
    let image = home.store(Config::default()).save_image(&input(png(&gradient(64, 64)), MediaType::Png)).expect("saved");
    let strict = Limits { max_image_bytes: 10, max_image_pixels: 4, max_image_dimension: 2, ..Limits::default() };
    let later = home.store(Config { limits: strict, ..Config::default() });
    assert!(later.read_image(&image, &Cancel::new()).is_ok());
}

#[test]
fn the_compression_concurrency_is_validated() {
    let home = Scratch::new("img-concurrency");
    for bad in [0, 9, 100] {
        assert!(vivianne_attachments::Store::new(home.path(), Config { compression_concurrency: bad, ..Config::default() }).is_err());
    }
    for good in [1, 2, 8] {
        assert!(vivianne_attachments::Store::new(home.path(), Config { compression_concurrency: good, ..Config::default() }).is_ok());
    }
}
