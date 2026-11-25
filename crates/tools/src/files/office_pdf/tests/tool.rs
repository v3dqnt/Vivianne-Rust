//! The `convert_to_pdf` tool over a made-up engine.

use serde_json::{Value, json};

use super::*;
use crate::Tool;
use crate::files::office_pdf::tool::ConvertToPdf;

fn folder() -> std::path::PathBuf {
    let dir = crate::testing::scratch("convert-tool");
    std::fs::canonicalize(&dir).expect("canonical")
}

fn tool(dir: &std::path::Path) -> (ConvertToPdf, Arc<Fake>) {
    let engine = Arc::new(Fake::default());
    (
        ConvertToPdf::with_service(dir, Arc::new(service(Config::default(), &engine))),
        engine,
    )
}

async fn run(tool: &ConvertToPdf, args: Value) -> crate::ToolResult {
    tool.execute(&args, "").await
}

#[tokio::test]
async fn a_document_becomes_a_pdf_beside_it_and_the_fonts_it_lacked_are_named() {
    let dir = folder();
    std::fs::write(dir.join("report.docx"), "hello").expect("doc");
    let (tool, engine) = tool(&dir);
    let done = run(&tool, json!({ "path": "report.docx" })).await;
    assert!(!done.is_error, "{}", done.content);
    assert!(done.content.contains("report.docx to report.pdf") && done.content.contains("Calibri"));
    assert!(
        std::fs::read(dir.join("report.pdf"))
            .expect("pdf")
            .starts_with(b"%PDF-")
    );
    assert_eq!(count(&engine.converts), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_output_can_be_named_and_goes_into_folders_that_do_not_exist_yet() {
    let dir = folder();
    std::fs::write(dir.join("deck.pptx"), "slides").expect("deck");
    let (tool, _) = tool(&dir);
    let done = run(
        &tool,
        json!({ "path": "deck.pptx", "output": "out/deck.pdf" }),
    )
    .await;
    assert!(!done.is_error, "{}", done.content);
    assert!(dir.join("out/deck.pdf").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_existing_pdf_is_kept_unless_the_model_says_to_replace_it() {
    let dir = folder();
    std::fs::write(dir.join("a.xlsx"), "cells").expect("xlsx");
    std::fs::write(dir.join("a.pdf"), "mine").expect("pdf");
    let (tool, engine) = tool(&dir);
    let refused = run(&tool, json!({ "path": "a.xlsx" })).await;
    assert!(refused.is_error && refused.content.contains("already exists"));
    assert_eq!(
        (
            std::fs::read(dir.join("a.pdf")).expect("pdf"),
            count(&engine.converts)
        ),
        (b"mine".to_vec(), 0),
        "nothing was converted for a refused overwrite"
    );
    assert!(
        !run(&tool, json!({ "path": "a.xlsx", "overwrite": true }))
            .await
            .is_error
    );
    assert!(
        std::fs::read(dir.join("a.pdf"))
            .expect("pdf")
            .starts_with(b"%PDF-")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn bad_requests_say_what_is_wrong_in_words_the_model_can_act_on() {
    let dir = folder();
    std::fs::write(dir.join("notes.txt"), "x").expect("txt");
    std::fs::write(dir.join("b.docx"), "x").expect("docx");
    let (tool, _) = tool(&dir);
    assert!(
        run(&tool, json!({}))
            .await
            .content
            .contains("requires a path")
    );
    assert!(
        run(&tool, json!({ "path": "notes.txt" }))
            .await
            .content
            .contains(".docx")
    );
    assert!(run(&tool, json!({ "path": "gone.docx" })).await.is_error);
    assert!(
        run(&tool, json!({ "path": "../outside.docx" }))
            .await
            .content
            .contains("outside the workspace")
    );
    assert!(
        run(&tool, json!({ "path": "b.docx", "output": "b.txt" }))
            .await
            .content
            .contains("must end in .pdf")
    );
    assert!(
        run(&tool, json!({ "path": "b.docx", "output": "../x.pdf" }))
            .await
            .content
            .contains("outside the workspace")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_engine_failure_is_reported_by_its_class_never_by_its_own_words() {
    let dir = folder();
    std::fs::write(dir.join("c.docx"), "x").expect("docx");
    let engine = Arc::new(Fake::default());
    *engine.fail.lock().expect("lock") = Some(
        Error::new(Code::InvalidDocument, "x").with_detail("soffice said: secret path C:/private"),
    );
    let tool = ConvertToPdf::with_service(&dir, Arc::new(service(Config::default(), &engine)));
    let failed = run(&tool, json!({ "path": "c.docx" })).await;
    assert!(failed.is_error && failed.content.contains("damaged or protected"));
    assert!(
        !failed.content.contains("secret"),
        "engine diagnostics stay out of what the model sees"
    );
    assert!(!dir.join("c.pdf").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_schema_asks_for_a_path_and_names_the_tool() {
    let definition = ConvertToPdf::new(std::path::Path::new("."))
        .definition()
        .expect("definition");
    assert_eq!(definition["name"], "convert_to_pdf");
    assert_eq!(definition["parameters"]["required"][0], "path");
}
