//! Verbatim files: exact bytes, content addressing, names, streaming, cancellation and verification on read.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::pedantic)] // tests assert by panicking

mod common;

use std::io::Read;

use common::Scratch;
use vivianne_attachments::{Cancel, Code, Config, Error};
use vivianne_core::attachment::AttachmentId;

fn read_all(
    store: &vivianne_attachments::Store,
    file: &vivianne_core::attachment::FileRef,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    for chunk in store.read_file_stream(file, &Cancel::new())? {
        out.extend(chunk?);
    }
    Ok(out)
}

#[test]
fn a_file_is_stored_read_only_at_a_digest_and_name_path_with_its_exact_bytes() {
    let home = Scratch::new("file-exact");
    let store = home.store(Config::default());
    let file = store
        .save_file(b"hello world", Some("C:\\Users\\ana\\notes.txt"))
        .expect("saved");
    assert_eq!((file.name.as_str(), file.bytes), ("notes.txt", 11));
    let path = store.file_host_path(&file).expect("path");
    assert!(
        path.ends_with(format!("{}/notes.txt", file.attachment_id.digest()))
            || path.ends_with("notes.txt")
    );
    assert_eq!(std::fs::read(&path).expect("readable"), b"hello world");
    assert!(
        std::fs::metadata(&path)
            .expect("meta")
            .permissions()
            .readonly()
    );
    assert_eq!(read_all(&store, &file).expect("verified"), b"hello world");
    // The reference holds the digest of exactly those bytes and no path.
    assert_eq!(
        AttachmentId::parse(file.attachment_id.as_str()).as_ref(),
        Some(&file.attachment_id)
    );
    assert!(
        !serde_json::to_string(&file)
            .expect("json")
            .contains("Users")
    );
}

#[test]
fn a_zero_byte_file_is_a_file() {
    let home = Scratch::new("file-empty");
    let store = home.store(Config::default());
    let file = store.save_file(b"", None).expect("saved");
    assert_eq!((file.name.as_str(), file.bytes), ("file", 0));
    assert!(read_all(&store, &file).expect("verified").is_empty());
}

#[test]
fn equal_bytes_are_stored_once_and_distinct_names_sit_beside_one_digest() {
    let home = Scratch::new("file-dedupe");
    let store = home.store(Config::default());
    let a = store.save_file(b"same", Some("a.txt")).expect("a");
    let b = store.save_file(b"same", Some("b.txt")).expect("b");
    let again = store.save_file(b"same", Some("a.txt")).expect("again");
    assert_eq!(a.attachment_id, b.attachment_id);
    assert_eq!(a, again);
    assert_ne!(
        store.file_host_path(&a).expect("a"),
        store.file_host_path(&b).expect("b")
    );
    let objects = store.layout().root.join("file-objects");
    let count = std::fs::read_dir(&objects)
        .expect("shards")
        .flatten()
        .flat_map(|s| std::fs::read_dir(s.path()).expect("objects").flatten())
        .count();
    assert_eq!(count, 1, "one object for one digest");
}

#[test]
fn reserved_and_multibyte_names_are_made_safe() {
    let home = Scratch::new("file-names");
    let store = home.store(Config::default());
    let device = store.save_file(b"x", Some("CON.txt")).expect("saved");
    assert_eq!(device.name, "_CON.txt");
    let multibyte = store
        .save_file(b"x", Some("日本語のファイル.txt"))
        .expect("saved");
    assert_eq!(multibyte.name, "日本語のファイル.txt");
    assert!(store.file_host_path(&multibyte).expect("path").exists());
}

#[test]
fn a_stored_object_that_no_longer_matches_its_digest_is_refused_not_trusted() {
    let home = Scratch::new("file-corrupt");
    let store = home.store(Config::default());
    let file = store.save_file(b"original", Some("a.txt")).expect("saved");
    let object = store.layout().file_object_path(file.attachment_id.digest());
    vivianne_attachments::objects::make_writable(&object).expect("writable");
    std::fs::write(&object, b"tampered").expect("tamper");
    // A second save of the same bytes meets the changed object and must not accept it.
    let again = store.save_file(b"original", Some("c.txt"));
    assert_eq!(
        again.map_err(|e| e.code()),
        Err(Some(Code::AttachmentCorrupt))
    );
    // And reading the first name fails on the last chunk.
    let outcome = read_all(&store, &file);
    assert_eq!(
        outcome.map_err(|e| e.code()),
        Err(Some(Code::AttachmentCorrupt))
    );
}

#[test]
fn a_changed_length_or_digest_in_the_reference_is_detected_on_read() {
    let home = Scratch::new("file-ref");
    let store = home.store(Config::default());
    let file = store
        .save_file(b"twelve bytes", Some("a.txt"))
        .expect("saved");
    let mut longer = file.clone();
    longer.bytes += 1;
    assert_eq!(
        read_all(&store, &longer).map_err(|e| e.code()),
        Err(Some(Code::AttachmentCorrupt))
    );
}

#[test]
fn malformed_references_and_unsafe_names_are_refused_before_any_path_is_built() {
    let home = Scratch::new("file-badref");
    let store = home.store(Config::default());
    let good = store.save_file(b"x", Some("a.txt")).expect("saved");
    for id in [
        "sha256:../../etc/passwd",
        "sha256:ABC",
        "nope",
        "",
        "sha256:",
    ] {
        let mut bad = good.clone();
        bad.attachment_id =
            serde_json::from_str(&format!("\"{id}\"")).expect("damaged id still reads");
        assert_eq!(
            store.file_host_path(&bad).map_err(|e| e.code()),
            Err(Some(Code::InvalidAttachmentRef)),
            "{id}"
        );
        assert!(store.read_file_stream(&bad, &Cancel::new()).is_err());
    }
    for name in ["../escape.txt", "a/b.txt", "con", "trailing.", ""] {
        let mut bad = good.clone();
        bad.name = name.to_owned();
        assert_eq!(
            store.file_host_path(&bad).map_err(|e| e.code()),
            Err(Some(Code::InvalidAttachmentRef)),
            "{name:?}"
        );
    }
}

#[test]
fn a_missing_file_is_not_found_and_not_a_crash() {
    let home = Scratch::new("file-missing");
    let store = home.store(Config::default());
    let file = store.save_file(b"x", Some("a.txt")).expect("saved");
    let path = store.file_host_path(&file).expect("path");
    vivianne_attachments::objects::make_writable(&path).expect("writable");
    std::fs::remove_file(&path).expect("removed");
    let opened = store.read_file_stream(&file, &Cancel::new());
    assert_eq!(
        opened.map(|_| ()).map_err(|e| e.code()),
        Err(Some(Code::AttachmentNotFound))
    );
}

/// A reader that hands out `chunks` of `size` bytes and records how large a read it was asked for.
struct Chunks {
    left: usize,
    size: usize,
    largest_ask: usize,
}

impl Read for Chunks {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.largest_ask = self.largest_ask.max(buf.len());
        let n = self.left.min(self.size).min(buf.len());
        buf[..n].fill(7);
        self.left -= n;
        Ok(n)
    }
}

#[test]
fn a_streamed_file_is_stored_without_one_aggregate_buffer() {
    let home = Scratch::new("file-stream");
    let store = home.store(Config::default());
    let total = 3 * 1024 * 1024 + 17;
    let mut source = Chunks {
        left: total,
        size: 50_000,
        largest_ask: 0,
    };
    let file = store
        .save_file_stream(&mut source, Some("big.bin"), &Cancel::new())
        .expect("stored");
    assert_eq!(file.bytes as usize, total);
    assert!(
        source.largest_ask <= 1 << 16,
        "the reader is asked for bounded chunks"
    );
    let mut got = 0usize;
    for chunk in store
        .read_file_stream(&file, &Cancel::new())
        .expect("opened")
    {
        let chunk = chunk.expect("verified");
        assert!(chunk.len() <= 1 << 16);
        got += chunk.len();
    }
    assert_eq!(got, total);
    // The stream and the one-shot save of the same bytes agree.
    let whole = store
        .save_file(&vec![7u8; total], Some("big.bin"))
        .expect("whole");
    assert_eq!(whole.attachment_id, file.attachment_id);
}

#[test]
fn cancelling_a_stream_removes_its_staging_file_and_publishes_nothing() {
    let home = Scratch::new("file-cancel");
    let store = home.store(Config::default());
    struct CancelsAfterFirst(Cancel, bool);
    impl Read for CancelsAfterFirst {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 {
                self.0.cancel();
            }
            self.1 = true;
            buf[..10].fill(1);
            Ok(10)
        }
    }
    let cancel = Cancel::new();
    let mut source = CancelsAfterFirst(cancel.clone(), false);
    let outcome = store.save_file_stream(&mut source, Some("x.bin"), &cancel);
    assert_eq!(outcome, Err(Error::Cancelled));
    let staging = store.layout().root.join("tmp");
    assert_eq!(std::fs::read_dir(&staging).map_or(0, Iterator::count), 0);
    assert!(!store.layout().root.join("files").exists());
}

#[test]
fn a_source_that_fails_is_a_write_failure_and_leaves_no_staging_file() {
    let home = Scratch::new("file-source-fails");
    let store = home.store(Config::default());
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("C:\\secret\\path failed"))
        }
    }
    let outcome = store.save_file_stream(&mut Broken, Some("x.bin"), &Cancel::new());
    let error = outcome.expect_err("failed");
    assert_eq!(error.code(), Some(Code::AttachmentWriteFailed));
    assert!(
        !error.to_string().contains("secret"),
        "no host path in a message"
    );
    assert_eq!(
        std::fs::read_dir(store.layout().root.join("tmp")).map_or(0, Iterator::count),
        0
    );
}

#[test]
fn a_file_over_the_size_cap_is_refused_as_it_streams_in() {
    let home = Scratch::new("file-cap");
    let config = Config {
        max_file_bytes: Some(1000),
        ..Config::default()
    };
    let store = home.store(config);
    assert!(store.save_file(&[0u8; 1000], None).is_ok());
    assert_eq!(
        store.save_file(&[0u8; 1001], None).map_err(|e| e.code()),
        Err(Some(Code::FileTooLarge))
    );
    // An endless source is stopped by the cap, not by memory.
    let mut endless = Chunks {
        left: usize::MAX,
        size: 4096,
        largest_ask: 0,
    };
    let outcome = store.save_file_stream(&mut endless, None, &Cancel::new());
    assert_eq!(outcome.map_err(|e| e.code()), Err(Some(Code::FileTooLarge)));
    assert_eq!(
        std::fs::read_dir(store.layout().root.join("tmp")).map_or(0, Iterator::count),
        0
    );
    let unlimited = home.store(Config {
        max_file_bytes: None,
        ..Config::default()
    });
    assert!(unlimited.save_file(&[0u8; 5000], None).is_ok());
}

#[test]
fn a_read_cancelled_before_or_during_stops_with_cancelled() {
    let home = Scratch::new("file-read-cancel");
    let store = home.store(Config::default());
    let file = store
        .save_file(&vec![1u8; 300_000], Some("a.bin"))
        .expect("saved");
    let cancel = Cancel::new();
    cancel.cancel();
    assert_eq!(
        store.read_file_stream(&file, &cancel).map(|_| ()),
        Err(Error::Cancelled)
    );
    let cancel = Cancel::new();
    let mut chunks = store.read_file_stream(&file, &cancel).expect("opened");
    assert!(chunks.next().expect("first").is_ok());
    cancel.cancel();
    assert_eq!(chunks.next().expect("second"), Err(Error::Cancelled));
    assert!(chunks.next().is_none());
}

#[test]
fn nothing_is_created_on_disk_until_something_is_saved() {
    let home = Scratch::new("file-lazy");
    let _store = home.store(Config::default());
    assert!(!home.path().join("attachments").exists());
}
