//! Admission, sharing, caching, priority, cancellation and disposal.

use super::*;

#[tokio::test]
async fn different_paths_with_the_same_bytes_are_read_twice_and_converted_once() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    let (a, b) = tokio::join!(
        office.convert(fg("a.docx", "same", &r), &cancel),
        office.convert(fg("b.docx", "same", &r), &cancel)
    );
    let (a, b) = (a.expect("a"), b.expect("b"));
    assert_eq!((count(&engine.converts), count(&r)), (1, 2));
    assert_eq!(a.cache_key, b.cache_key);
    assert_eq!(a.missing_fonts, ["Calibri"]);
    assert_eq!(a.generation, office.generation());
}

#[tokio::test]
async fn a_known_source_is_not_read_again_and_every_result_owns_its_buffers() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    let mut first = office
        .convert(fg("a.docx", "doc", &r), &cancel)
        .await
        .expect("first");
    first.pdf.clear();
    first.missing_fonts.clear();
    let second = office
        .convert(fg("a.docx", "doc", &r), &cancel)
        .await
        .expect("second");
    assert_eq!((count(&engine.converts), count(&r)), (1, 1));
    assert!(!second.pdf.is_empty() && second.missing_fonts == ["Calibri"]);
}

#[tokio::test]
async fn an_extension_and_a_source_version_each_make_their_own_conversion() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    office
        .convert(
            request(
                "a",
                Mem::new("x", "1", &r),
                Extension::Doc,
                Priority::Foreground,
                Some(1),
            ),
            &cancel,
        )
        .await
        .expect("doc");
    office
        .convert(
            request(
                "a",
                Mem::new("x", "1", &r),
                Extension::Docx,
                Priority::Foreground,
                Some(1),
            ),
            &cancel,
        )
        .await
        .expect("docx");
    assert_eq!(
        count(&engine.converts),
        2,
        "the same bytes under another extension are another document"
    );
    office
        .convert(
            request(
                "a",
                Mem::new("y", "2", &r),
                Extension::Docx,
                Priority::Foreground,
                Some(1),
            ),
            &cancel,
        )
        .await
        .expect("v2");
    assert_eq!(count(&engine.converts), 3);
}

#[tokio::test]
async fn a_stat_size_over_the_limit_is_refused_before_any_read() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(
        Config {
            max_input_bytes: 10,
            max_source_bytes: 10,
            ..Config::default()
        },
        &engine,
    );
    let big = request(
        "a",
        Mem::new("0123456789ab", "1", &r),
        Extension::Docx,
        Priority::Foreground,
        Some(12),
    );
    assert_eq!(
        office
            .convert(big, &CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        Code::InputTooLarge
    );
    assert_eq!(count(&r), 0);
}

#[tokio::test]
async fn an_unknown_size_reserves_the_whole_input_limit() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(
        Config {
            max_input_bytes: 100,
            max_source_bytes: 100,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let unknown = |key: &str, content: &str| {
        request(
            key,
            Mem::new(content, "1", &r),
            Extension::Docx,
            Priority::Foreground,
            None,
        )
    };
    let (a, b) = tokio::join!(
        office.convert(unknown("a", "one"), &cancel),
        office.convert(unknown("b", "two"), &cancel)
    );
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(
        engine.max_active.load(Ordering::SeqCst),
        1,
        "two unknown sizes cannot both hold the capacity"
    );
}

#[tokio::test]
async fn a_version_that_changed_or_a_read_that_overflowed_is_refused_and_not_kept() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(
        Config {
            max_input_bytes: 8,
            max_source_bytes: 8,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let mut moved = Mem::new("abc", "1", &r);
    moved.answers_version = Some("2".into());
    let stale = request("a", moved, Extension::Docx, Priority::Foreground, Some(3));
    assert_eq!(
        office.convert(stale, &cancel).await.unwrap_err().code,
        Code::SourceChanged
    );
    let grown = request(
        "b",
        Mem::new("0123456789", "1", &r),
        Extension::Docx,
        Priority::Foreground,
        Some(3),
    );
    assert_eq!(
        office.convert(grown, &cancel).await.unwrap_err().code,
        Code::InputTooLarge
    );
    assert_eq!(count(&engine.converts), 0);
    office
        .convert(fg("a", "abc", &r), &cancel)
        .await
        .expect("the right version converts");
    assert_eq!(count(&engine.converts), 1);
}

#[tokio::test]
async fn the_least_recently_used_pdf_leaves_with_its_aliases() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(
        Config {
            max_cached_entries: 2,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    for (key, content) in [("a", "A"), ("b", "B"), ("c", "C")] {
        office
            .convert(fg(key, content, &r), &cancel)
            .await
            .expect("converts");
    }
    assert_eq!(count(&r), 3);
    office
        .convert(fg("c", "C", &r), &cancel)
        .await
        .expect("c is still kept");
    assert_eq!(
        count(&r),
        3,
        "the newest alias still answers without a read"
    );
    office
        .convert(fg("a", "A", &r), &cancel)
        .await
        .expect("a is converted again");
    assert_eq!(
        (count(&r), count(&engine.converts)),
        (4, 4),
        "the oldest PDF and its alias were evicted"
    );
}

#[tokio::test]
async fn failures_and_pdfs_over_the_retention_budget_are_not_kept() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    *engine.fail.lock().expect("lock") = Some(Error::new(Code::InvalidDocument, "bad"));
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    assert_eq!(
        office
            .convert(fg("a", "x", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::InvalidDocument
    );
    *engine.fail.lock().expect("lock") = None;
    office
        .convert(fg("a", "x", &r), &cancel)
        .await
        .expect("a retry is a new attempt");
    assert_eq!(count(&r), 2);

    let (engine, r) = (Arc::new(Fake::default()), reads());
    let tiny = service(
        Config {
            max_cached_bytes: 4,
            ..Config::default()
        },
        &engine,
    );
    tiny.convert(fg("a", "x", &r), &cancel)
        .await
        .expect("converts");
    tiny.convert(fg("a", "x", &r), &cancel)
        .await
        .expect("converts again");
    assert_eq!(
        count(&engine.converts),
        2,
        "a PDF over the budget is never kept"
    );
}

#[tokio::test]
async fn reader_slots_are_bounded_and_the_last_one_is_kept_for_foreground_work() {
    let (engine, gate) = Fake::gated();
    let r = reads();
    let office = service(
        Config {
            max_readers: 2,
            max_concurrent_conversions: 1,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let first = tokio::spawn({
        let (office, cancel, req) = (office.clone(), cancel.clone(), fg("a", "A", &r));
        async move { office.convert(req, &cancel).await }
    });
    settle().await;
    // One slot left: speculation may not take it, foreground work may.
    assert_eq!(
        office
            .convert(bg("b", "B", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Busy
    );
    let second = tokio::spawn({
        let (office, cancel, req) = (office.clone(), cancel.clone(), fg("c", "C", &r));
        async move { office.convert(req, &cancel).await }
    });
    settle().await;
    assert_eq!(
        office
            .convert(fg("d", "D", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Busy
    );
    gate.add_permits(2);
    assert!(first.await.expect("joins").is_ok() && second.await.expect("joins").is_ok());
}

#[tokio::test]
async fn disabled_prewarming_is_refused_before_reading_but_ready_pdfs_still_serve_it() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(
        Config {
            max_background_conversions: 0,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    assert_eq!(
        office
            .convert(bg("a", "A", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Busy
    );
    assert_eq!(count(&r), 0);
    office
        .convert(fg("a", "A", &r), &cancel)
        .await
        .expect("foreground works");
    assert!(
        office.convert(bg("a", "A", &r), &cancel).await.is_ok(),
        "a completed alias hit needs no reader slot"
    );
}

#[tokio::test]
async fn the_queue_is_bounded_and_foreground_work_displaces_queued_speculation() {
    let (engine, gate) = Fake::gated();
    let r = reads();
    let office = service(
        Config {
            max_concurrent_conversions: 1,
            max_queued_jobs: 1,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let spawn = |req: Request| {
        let (office, cancel) = (office.clone(), cancel.clone());
        tokio::spawn(async move { office.convert(req, &cancel).await })
    };
    let running = spawn(fg("run", "R", &r));
    settle().await;
    let speculative = spawn(bg("spec", "S", &r));
    settle().await;
    assert_eq!(
        office
            .convert(bg("more", "M", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Busy,
        "speculation cannot displace anything"
    );
    let wanted = spawn(fg("want", "W", &r));
    assert_eq!(
        speculative.await.expect("joins").unwrap_err().code,
        Code::Busy,
        "the queued speculation made room"
    );
    gate.add_permits(2);
    assert!(running.await.expect("joins").is_ok() && wanted.await.expect("joins").is_ok());
    assert_eq!(count(&r), 2, "the displaced source was never read");
}

#[tokio::test]
async fn background_work_leaves_a_slot_for_foreground_work() {
    let (engine, gate) = Fake::gated();
    let r = reads();
    let office = service(
        Config {
            max_concurrent_conversions: 2,
            max_background_conversions: 2,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let spawn = |req: Request| {
        let (office, cancel) = (office.clone(), cancel.clone());
        tokio::spawn(async move { office.convert(req, &cancel).await })
    };
    let handles = [spawn(bg("a", "A", &r)), spawn(bg("b", "B", &r))];
    settle().await;
    assert_eq!(
        engine.active.load(Ordering::SeqCst),
        1,
        "only one speculative job runs while there are two slots"
    );
    let foreground = spawn(fg("f", "F", &r));
    settle().await;
    assert_eq!(
        engine.active.load(Ordering::SeqCst),
        2,
        "the foreground job took the other slot"
    );
    gate.add_permits(3);
    for handle in handles {
        assert!(handle.await.expect("joins").is_ok());
    }
    assert!(foreground.await.expect("joins").is_ok());
}

#[tokio::test]
async fn a_foreground_reader_promotes_queued_speculation_without_another_read() {
    let (engine, gate) = Fake::gated();
    let r = reads();
    let office = service(
        Config {
            max_concurrent_conversions: 1,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let spawn = |req: Request| {
        let (office, cancel) = (office.clone(), cancel.clone());
        tokio::spawn(async move { office.convert(req, &cancel).await })
    };
    let running = spawn(fg("run", "R", &r));
    settle().await;
    let prewarm = spawn(bg("doc", "D", &r));
    settle().await;
    let joined = spawn(fg("doc", "D", &r));
    settle().await;
    gate.add_permits(2);
    assert!(
        running.await.expect("joins").is_ok()
            && prewarm.await.expect("joins").is_ok()
            && joined.await.expect("joins").is_ok()
    );
    assert_eq!(
        count(&r),
        2,
        "one read for the running source and one shared by both readers of the other"
    );
    assert_eq!(count(&engine.converts), 2);
}

#[tokio::test]
async fn one_reader_leaving_keeps_the_conversion_and_the_last_one_cancels_it() {
    let (engine, gate) = Fake::gated();
    let r = reads();
    let office = service(Config::default(), &engine);
    let (first, second) = (CancellationToken::new(), CancellationToken::new());
    let spawn = |cancel: &CancellationToken| {
        let (office, cancel, req) = (office.clone(), cancel.clone(), fg("a", "A", &r));
        tokio::spawn(async move { office.convert(req, &cancel).await })
    };
    let (a, b) = (spawn(&first), spawn(&second));
    settle().await;
    first.cancel();
    assert_eq!(a.await.expect("joins").unwrap_err().code, Code::Cancelled);
    settle().await;
    assert_eq!(
        count(&engine.cancelled),
        0,
        "the other reader still wants it"
    );
    gate.add_permits(1);
    assert!(b.await.expect("joins").is_ok());

    let (engine, _gate) = Fake::gated();
    let office = service(Config::default(), &engine);
    let only = CancellationToken::new();
    let lone = tokio::spawn({
        let (office, only, req) = (office.clone(), only.clone(), fg("z", "Z", &r));
        async move { office.convert(req, &only).await }
    });
    settle().await;
    only.cancel();
    assert_eq!(
        lone.await.expect("joins").unwrap_err().code,
        Code::Cancelled
    );
    settle().await;
    assert_eq!(
        count(&engine.cancelled),
        1,
        "with nobody left, the engine was told to stop"
    );
}

#[tokio::test]
async fn capacity_stays_held_until_cancelled_engine_work_has_settled() {
    let (engine, gate) = Fake::gated();
    let r = reads();
    let office = service(
        Config {
            max_concurrent_conversions: 1,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let first = CancellationToken::new();
    let a = tokio::spawn({
        let (office, first, req) = (office.clone(), first.clone(), fg("a", "A", &r));
        async move { office.convert(req, &first).await }
    });
    settle().await;
    let b = tokio::spawn({
        let (office, cancel, req) = (office.clone(), cancel.clone(), fg("b", "B", &r));
        async move { office.convert(req, &cancel).await }
    });
    first.cancel();
    assert!(a.await.expect("joins").is_err());
    gate.add_permits(1);
    assert!(
        b.await.expect("joins").is_ok(),
        "the queued job ran once the cancelled one had settled"
    );
}

#[tokio::test]
async fn disposal_tells_outstanding_readers_it_is_unavailable_and_refuses_new_ones() {
    let (engine, _gate) = Fake::gated();
    let r = reads();
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    let waiting = tokio::spawn({
        let (office, cancel, req) = (office.clone(), cancel.clone(), fg("a", "A", &r));
        async move { office.convert(req, &cancel).await }
    });
    settle().await;
    office.dispose().await;
    assert_eq!(
        waiting.await.expect("joins").unwrap_err().code,
        Code::Unavailable
    );
    assert_eq!(
        office
            .convert(fg("b", "B", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Unavailable
    );
    assert_eq!(
        engine.active.load(Ordering::SeqCst),
        0,
        "disposal waited for the engine"
    );
}

#[tokio::test]
async fn a_caller_that_is_already_cancelled_allocates_nothing() {
    let (engine, r) = (Arc::new(Fake::default()), reads());
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        office
            .convert(fg("a", "A", &r), &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Cancelled
    );
    assert_eq!((count(&r), count(&engine.converts)), (0, 0));
}
