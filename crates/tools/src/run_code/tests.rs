//! Programs run against a real folder with the real read-only tools.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;

use super::*;

fn project() -> PathBuf {
    let dir = crate::testing::scratch("run-code");
    std::fs::create_dir_all(dir.join("src")).expect("dir");
    std::fs::write(dir.join("src/a.rs"), "fn a() {}\n// TODO one\n").expect("a");
    std::fs::write(
        dir.join("src/b.rs"),
        "fn b() {}\n// TODO two\n// TODO three\n",
    )
    .expect("b");
    std::fs::write(dir.join("notes.txt"), "plain\n").expect("notes");
    dir
}

async fn run_in(dir: &Path, code: &str) -> ToolResult {
    RunCode::new(dir)
        .execute(&json!({ "code": code }), "")
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_returns_a_value_and_prints_what_it_logs() {
    let dir = project();
    let out = run_in(
        &dir,
        "console.log('hello', 1 + 1, {a: [1]}); return { sum: 40 + 2 };",
    )
    .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "hello 2 {\"a\":[1]}\nresult: {\"sum\":42}");
    assert_eq!(
        run_in(&dir, "console.log('only prints');").await.content,
        "only prints"
    );
    assert!(run_in(&dir, "var x = 1;").await.content.contains("nothing"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_reads_files_through_the_tools_and_answers_with_only_what_it_chose() {
    let dir = project();
    let out = run_in(
        &dir,
        "const files = ['src/a.rs', 'src/b.rs'];\nlet todos = 0;\nfor (const f of files) { todos += (await tools.read({ path: f })).split('\\n').filter(l => l.includes('TODO')).length; }\nreturn todos;",
    )
    .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "result: 3");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_started_together_run_together_and_come_back_in_order() {
    let dir = project();
    let out = run_in(&dir, "const [a, b] = await Promise.all([tools.read({ path: 'src/a.rs' }), tools.read({ path: 'src/b.rs' })]);\nreturn [a.includes('fn a'), b.includes('fn b')];").await;
    assert_eq!(out.content, "result: [true,true]", "{}", out.content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_call_rejects_with_a_tool_error_the_program_may_catch() {
    let dir = project();
    let caught = run_in(&dir, "try { await tools.read({ path: 'nope.rs' }); return 'no error'; } catch (e) { return [e instanceof ToolError, e.tool]; }").await;
    assert_eq!(
        caught.content, "result: [true,\"read\"]",
        "{}",
        caught.content
    );
    let uncaught = run_in(&dir, "await tools.read({ path: 'nope.rs' });").await;
    assert!(
        uncaught.is_error
            && uncaught.content.contains("the program failed (exception)")
            && uncaught.content.contains("ToolError"),
        "{}",
        uncaught.content
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_is_checked_against_the_tools_schema_and_only_read_only_tools_exist() {
    let dir = project();
    let missing = run_in(
        &dir,
        "try { await tools.read({}); } catch (e) { return e.message; }",
    )
    .await;
    assert!(
        missing.content.contains("invalid arguments") && missing.content.contains("path"),
        "{}",
        missing.content
    );
    for tool in ["shell", "write", "edit", "all"] {
        assert_eq!(
            run_in(&dir, &format!("return typeof tools.{tool};"))
                .await
                .content,
            "result: \"undefined\"",
            "{tool}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_has_a_limit_on_how_many_calls_it_makes() {
    let dir = project();
    let tool = RunCode::new(&dir).with_limits(TIME_LIMIT, 3);
    let out = tool.execute(&json!({ "code": "let n = 0; try { for (let i = 0; i < 10; i++) { await tools.read({ path: 'notes.txt' }); n++; } } catch (e) { return [n, e.message]; }" }), "").await;
    assert!(
        out.content
            .contains("[3,\"this program has used its 3 tool calls\"]"),
        "{}",
        out.content
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_never_ends_is_stopped() {
    let dir = project();
    let tool = RunCode::new(&dir).with_limits(Duration::from_millis(300), MAX_CALLS);
    let started = Instant::now();
    // The tool's own limit is the runtime's cap, so ask for less through the request: run the loop under a short budget.
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        tool.execute(&json!({ "code": "while (true) {}" }), ""),
    )
    .await
    .expect("the program was stopped");
    assert!(
        result.is_error && result.content.contains("timeout"),
        "{}",
        result.content
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_or_empty_code_comes_back_as_an_error() {
    let dir = project();
    let broken = run_in(&dir, "return (;").await;
    assert!(
        broken.is_error && broken.content.starts_with("the program failed"),
        "{}",
        broken.content
    );
    let none = RunCode::new(&dir)
        .execute(&json!({ "code": "   " }), "")
        .await;
    assert!(none.is_error && none.content.contains("requires code"));
    assert!(RunCode::new(&dir).execute(&json!({}), "").await.is_error);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_retained_state_variables_survive_from_one_call_to_the_next() {
    let dir = project();
    let tool = RunCode::with_mode(&dir, ExecutionMode::Persistent);
    let first = tool
        .execute(
            &json!({ "code": "var seen = (await_free = 1, 1); seen" }),
            "",
        )
        .await;
    assert_eq!(first.content, "result: 1", "{}", first.content);
    let second = tool
        .execute(&json!({ "code": "seen += 1; seen" }), "")
        .await;
    assert_eq!(
        second.content, "result: 2",
        "the variable from the first call is still there: {}",
        second.content
    );
    // A promise as the last value is waited for, so tools can be used without a top-level await.
    let third = tool.execute(&json!({ "code": "tools.read({ path: 'notes.txt' }).then((text) => text.includes('plain'))" }), "").await;
    assert_eq!(third.content, "result: true", "{}", third.content);
    let fresh = RunCode::with_mode(&dir, ExecutionMode::Persistent);
    assert_eq!(
        fresh
            .execute(&json!({ "code": "typeof seen" }), "")
            .await
            .content,
        "result: \"undefined\"",
        "each toolbox has its own environment"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_description_tells_how_to_write_a_program_and_lists_the_tools_with_their_arguments() {
    let dir = project();
    let definition = RunCode::new(&dir).definition().expect("definition");
    let text = definition["description"].as_str().expect("description");
    assert!(
        text.contains("await tools.read({ path,")
            && text.contains("limit?")
            && text.contains("await tools.grep({ pattern, path?")
            && text.contains("await tools.glob("),
        "{text}"
    );
    assert!(
        text.contains("Promise.all")
            && text.contains("ToolError")
            && text.contains("async function"),
        "{text}"
    );
    assert!(!text.contains("tools.shell") && !text.contains("tools.write"));
    assert_eq!(definition["parameters"]["required"][0], "code");
    let persistent = RunCode::with_mode(&dir, ExecutionMode::Persistent)
        .definition()
        .expect("definition");
    assert!(
        persistent["description"]
            .as_str()
            .expect("description")
            .contains("stay for the next run")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
