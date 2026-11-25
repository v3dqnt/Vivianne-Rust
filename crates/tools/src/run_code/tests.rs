//! Programs run against a real folder with the real read-only tools.

use std::path::PathBuf;

use serde_json::json;

use super::*;

fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viv-run-code-{}", uuid_like()));
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

fn uuid_like() -> String {
    format!(
        "{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    )
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
        "const files = ['src/a.rs', 'src/b.rs'];\nlet todos = 0;\nfor (const f of files) { todos += tools.read({ path: f }).split('\\n').filter(l => l.includes('TODO')).length; }\nreturn todos;",
    )
    .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "result: 3");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn several_calls_can_run_at_once_and_come_back_in_order() {
    let dir = project();
    let out = run_in(&dir, "const [a, b] = tools.all([['read', { path: 'src/a.rs' }], ['read', { path: 'src/b.rs' }]]);\nreturn [a.includes('fn a'), b.includes('fn b')];").await;
    assert_eq!(out.content, "result: [true,true]", "{}", out.content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_call_throws_an_error_the_program_may_catch() {
    let dir = project();
    let caught = run_in(&dir, "try { tools.read({ path: 'nope.rs' }); return 'no error'; } catch (e) { return 'caught: ' + (e instanceof Error); }").await;
    assert_eq!(caught.content, "result: \"caught: true\"");
    let uncaught = run_in(&dir, "tools.read({ path: 'nope.rs' });").await;
    assert!(
        uncaught.is_error && uncaught.content.contains("the program failed"),
        "{}",
        uncaught.content
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_is_checked_against_the_tools_schema_and_an_unknown_tool_is_refused() {
    let dir = project();
    let missing = run_in(
        &dir,
        "try { tools.read({}); } catch (e) { return e.message; }",
    )
    .await;
    assert!(
        missing.content.contains("invalid arguments") && missing.content.contains("path"),
        "{}",
        missing.content
    );
    let unknown = run_in(
        &dir,
        "try { tools.all([['shell', { command: 'echo hi' }]]); } catch (e) { return e.message; }",
    )
    .await;
    assert!(
        unknown.content.contains("there is no tool `shell`"),
        "{}",
        unknown.content
    );
    assert!(
        run_in(&dir, "return typeof tools.shell;")
            .await
            .content
            .contains("undefined"),
        "only read-only tools exist in a program"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_has_a_limit_on_how_many_calls_it_makes() {
    let dir = project();
    let tool = RunCode::new(&dir).with_limits(TIME_LIMIT, 3);
    let out = tool.execute(&json!({ "code": "let n = 0; try { for (let i = 0; i < 10; i++) { tools.read({ path: 'notes.txt' }); n++; } } catch (e) { return [n, e.message]; }" }), "").await;
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
    let out = tool
        .execute(&json!({ "code": "while (true) {}" }), "")
        .await;
    assert!(
        out.is_error && out.content.contains("program failed"),
        "{}",
        out.content
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_eats_memory_or_stack_fails_instead_of_taking_the_app_down() {
    let dir = project();
    let memory = run_in(
        &dir,
        "const big = []; while (true) { big.push('x'.repeat(1 << 20)); }",
    )
    .await;
    assert!(memory.is_error, "{}", memory.content);
    let stack = run_in(&dir, "function f(n) { return f(n + 1) + 1; } return f(0);").await;
    assert!(stack.is_error, "{}", stack.content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_has_no_way_out_except_the_tools() {
    let dir = project();
    for probe in [
        "typeof require",
        "typeof process",
        "typeof fetch",
        "typeof XMLHttpRequest",
        "typeof Deno",
        "typeof std",
        "typeof os",
    ] {
        let out = run_in(&dir, &format!("return {probe};")).await;
        assert_eq!(
            out.content, "result: \"undefined\"",
            "{probe}: {}",
            out.content
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_errors_and_empty_code_come_back_as_errors() {
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
async fn a_huge_output_is_cut() {
    let dir = project();
    let out = run_in(&dir, "console.log('y'.repeat(200000));").await;
    assert!(
        out.content.len() <= MAX_OUTPUT_BYTES + 32 && out.content.ends_with("[output cut]"),
        "{}",
        out.content.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_description_lists_the_tools_a_program_may_call_with_their_arguments() {
    let dir = project();
    let tool = RunCode::new(&dir);
    let definition = tool.definition().expect("definition");
    let text = definition["description"].as_str().expect("description");
    assert!(
        text.contains("tools.read({ path, limit?")
            && text.contains("tools.grep({ pattern, path?")
            && text.contains("tools.glob("),
        "{text}"
    );
    assert!(!text.contains("tools.shell") && !text.contains("tools.write"));
    assert_eq!(definition["parameters"]["required"][0], "code");
    let _ = std::fs::remove_dir_all(&dir);
}
