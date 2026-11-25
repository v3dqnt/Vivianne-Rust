//! Limits, extensions, and rendering a file from disk.

use super::*;

#[test]
fn limits_must_be_usable() {
    assert!(Config::default().validate().is_ok());
    assert!(
        Config {
            max_readers: 0,
            ..Config::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            max_source_bytes: 1,
            ..Config::default()
        }
        .validate()
        .is_err(),
        "capacity must cover one input"
    );
    assert!(
        Config {
            max_background_conversions: 0,
            ..Config::default()
        }
        .validate()
        .is_ok()
    );
    assert!(
        OfficeToPdf::new(
            Config {
                timeout_ms: 0,
                ..Config::default()
            },
            Arc::new(Fake::default())
        )
        .is_err()
    );
}

#[test]
fn only_office_extensions_are_accepted_whatever_their_case_or_folder() {
    assert_eq!(
        Extension::of_path("C:\\docs\\Report.DOCX"),
        Some(Extension::Docx)
    );
    assert_eq!(Extension::of_path("a/b/sheet.xls"), Some(Extension::Xls));
    assert_eq!(Extension::of_path("deck.pptx"), Some(Extension::Pptx));
    assert_eq!(Extension::of_path("notes.txt"), None);
    assert_eq!(Extension::of_path("docx"), None);
    assert_eq!(Extension::of_path("dir.docx/readme"), None);
}

fn temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let dir = crate::testing::scratch("office-test");
    let path = dir.join(name);
    std::fs::write(&path, content).expect("write");
    path
}

#[tokio::test]
async fn a_file_is_rendered_with_its_identity_and_unsupported_or_missing_paths_are_refused() {
    let engine = Arc::new(Fake::default());
    let office = service(Config::default(), &engine);
    let cancel = CancellationToken::new();
    let path = temp_file("report.docx", "hello");
    let done = office
        .render_file(&path, Priority::Foreground, &cancel)
        .await
        .expect("renders");
    assert!(
        done.path.ends_with("report.docx")
            && done.pdf.starts_with(b"%PDF-")
            && !done.version.is_empty()
    );
    assert_eq!(done.generation, office.generation());
    let again = office
        .render_file(&path, Priority::Foreground, &cancel)
        .await
        .expect("again");
    assert_eq!(
        (count(&engine.converts), again.version),
        (1, done.version),
        "an unchanged file is served from what was kept"
    );

    let text = temp_file("notes.txt", "x");
    assert_eq!(
        office
            .render_file(&text, Priority::Foreground, &cancel)
            .await
            .unwrap_err()
            .code,
        Code::UnsupportedFormat
    );
    let missing = path.with_file_name("gone.docx");
    assert_eq!(
        office
            .render_file(&missing, Priority::Foreground, &cancel)
            .await
            .unwrap_err()
            .code,
        Code::Failed
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    let _ = std::fs::remove_dir_all(text.parent().expect("parent"));
}

#[tokio::test]
async fn a_file_that_changes_while_it_waits_is_source_changed_and_a_too_big_one_is_refused() {
    let (engine, gate) = Fake::gated();
    let office = service(
        Config {
            max_concurrent_conversions: 1,
            max_input_bytes: 16,
            max_source_bytes: 16,
            ..Config::default()
        },
        &engine,
    );
    let cancel = CancellationToken::new();
    let r = reads();
    let blocker = tokio::spawn({
        let (office, cancel, req) = (office.clone(), cancel.clone(), fg("blocker", "B", &r));
        async move { office.convert(req, &cancel).await }
    });
    settle().await;
    let path = temp_file("moving.docx", "before");
    let waiting = tokio::spawn({
        let (office, cancel, path) = (office.clone(), cancel.clone(), path.clone());
        async move {
            office
                .render_file(&path, Priority::Foreground, &cancel)
                .await
        }
    });
    settle().await;
    std::fs::write(&path, "after, and longer").expect("edit");
    gate.add_permits(2);
    assert!(blocker.await.expect("joins").is_ok());
    assert_eq!(
        waiting.await.expect("joins").unwrap_err().code,
        Code::SourceChanged
    );

    let big = temp_file("big.docx", &"x".repeat(64));
    assert_eq!(
        office
            .render_file(&big, Priority::Foreground, &cancel)
            .await
            .unwrap_err()
            .code,
        Code::InputTooLarge
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    let _ = std::fs::remove_dir_all(big.parent().expect("parent"));
}
