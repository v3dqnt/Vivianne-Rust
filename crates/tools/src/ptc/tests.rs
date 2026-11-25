//! The runtime against real programs and real (made-up) host functions.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;

fn function<F, Fut>(f: F) -> BindingFunction
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, String>> + Send + 'static,
{
    Arc::new(move |args| Box::pin(f(args)))
}

fn namespace(
    global: &str,
    functions: Vec<(&str, BindingFunction)>,
    class: Option<(&str, &str)>,
) -> BindingNamespace {
    BindingNamespace {
        global: global.to_owned(),
        functions: functions
            .into_iter()
            .map(|(name, f)| (name.to_owned(), f))
            .collect::<BTreeMap<_, _>>(),
        error_class: class.map(|(name, property)| BindingErrorClass {
            name: name.to_owned(),
            member_name_property: property.to_owned(),
        }),
    }
}

/// `tools.echo(x)` answers x; `tools.fail()` rejects; `tools.slow(ms)` waits that long.
fn tools() -> BindingNamespace {
    namespace(
        "tools",
        vec![
            ("echo", function(|args| async move { Ok(args) })),
            (
                "fail",
                function(|_| async move { Err("it did not work".to_owned()) }),
            ),
            (
                "slow",
                function(|args| async move {
                    tokio::time::sleep(Duration::from_millis(args.as_u64().unwrap_or(0))).await;
                    Ok(json!("slept"))
                }),
            ),
            (
                "__proto__",
                function(|_| async move { Ok(json!("own property")) }),
            ),
            (
                "constructor",
                function(|_| async move { Ok(json!("own constructor")) }),
            ),
        ],
        Some(("ToolError", "tool")),
    )
}

fn runtime(mode: ExecutionMode) -> Runtime {
    Runtime::new(Config {
        mode,
        ..Config::default()
    })
    .expect("runtime")
}

async fn go(runtime: &Runtime, program: &str) -> RunResult {
    go_with(
        runtime,
        RunRequest {
            program: program.to_owned(),
            bindings: vec![tools()],
            ..RunRequest::default()
        },
    )
    .await
}

async fn go_with(runtime: &Runtime, request: RunRequest) -> RunResult {
    let spec = runtime.resolve(request).expect("resolves");
    runtime.run(spec).await.expect("runs")
}

fn error_of(result: &RunResult) -> &Failure {
    result
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("expected a failure, got {result:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_function_body_returns_its_value_and_logs_in_order() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(
        &rt,
        "console.log('a', 1, {b: [2]}); console.warn('w'); return { sum: 1 + 2 };",
    )
    .await;
    assert_eq!(result.error, None);
    assert_eq!(result.value, Some(json!({ "sum": 3 })));
    assert_eq!(result.logs, ["a 1 {\"b\":[2]}", "w"]);
    let none = go(&rt, "console.log('only'); ").await;
    assert_eq!((none.value, none.logs), (None, vec!["only".to_owned()]));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_functions_are_awaited_and_their_values_come_back_as_json() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(&rt, "const a = await tools.echo({ n: 1, list: [true, null, 'x'] }); const b = await tools.echo(undefined); return [a, b];").await;
    assert_eq!(
        result.value,
        Some(json!([{ "n": 1, "list": [true, null, "x"] }, null])),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_started_together_run_together() {
    let rt = runtime(ExecutionMode::PerCall);
    let started = Instant::now();
    let result = go(&rt, "const all = await Promise.all([tools.slow(300), tools.slow(300), tools.slow(300)]); return all.length;").await;
    assert_eq!(result.value, Some(json!(3)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "three 300 ms waits took {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_call_is_an_instance_of_the_typed_error_that_names_the_member() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(&rt, "try { await tools.fail(); } catch (e) { return [e instanceof ToolError, e instanceof Error, e.name, e.message, e.tool]; }").await;
    assert_eq!(
        result.value,
        Some(json!([true, true, "ToolError", "it did not work", "fail"])),
        "{result:?}"
    );
    let uncaught = go(&rt, "await tools.fail();").await;
    let failure = error_of(&uncaught);
    assert_eq!(failure.kind, FailureKind::Exception);
    assert!(
        failure.message.contains("ToolError: it did not work"),
        "{}",
        failure.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn member_names_that_are_prototype_slots_are_ordinary_own_properties() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(&rt, "return [await tools['__proto__'](), await tools.constructor(), Object.getPrototypeOf(tools) === null];").await;
    assert_eq!(
        result.value,
        Some(json!(["own property", "own constructor", true])),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exceptions_and_syntax_errors_fail_as_exceptions_with_what_the_model_needs() {
    let rt = runtime(ExecutionMode::PerCall);
    let thrown = go(&rt, "throw new RangeError('too far');").await;
    assert!(
        error_of(&thrown).message.contains("RangeError: too far")
            && error_of(&thrown).kind == FailureKind::Exception,
        "{thrown:?}"
    );
    let syntax = go(&rt, "return (;").await;
    assert_eq!(error_of(&syntax).kind, FailureKind::Exception);
    let plain = go(&rt, "throw 'just text';").await;
    assert!(error_of(&plain).message.contains("just text"), "{plain:?}");
    let logs_kept = go(&rt, "console.log('before'); throw new Error('x');").await;
    assert_eq!(
        logs_kept.logs,
        ["before"],
        "what was printed before the failure is kept"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_with_no_way_out_has_only_its_bindings() {
    let rt = runtime(ExecutionMode::PerCall);
    for probe in [
        "require",
        "process",
        "fetch",
        "XMLHttpRequest",
        "Deno",
        "std",
        "os",
        "WebSocket",
    ] {
        let result = go(&rt, &format!("return typeof {probe};")).await;
        assert_eq!(result.value, Some(json!("undefined")), "{probe}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deadline_stops_a_loop_and_says_whether_the_code_had_started() {
    let rt = runtime(ExecutionMode::PerCall);
    let started = Instant::now();
    let result = go_with(
        &rt,
        RunRequest {
            program: "while (true) {}".into(),
            timeout: Timeout::Ms(250),
            ..RunRequest::default()
        },
    )
    .await;
    let failure = error_of(&result);
    assert_eq!(failure.kind, FailureKind::Timeout);
    assert!(
        failure.message.contains("timed out during execution") && failure.message.contains("250"),
        "{}",
        failure.message
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_deadline_covers_time_spent_waiting_for_host_functions() {
    let rt = runtime(ExecutionMode::PerCall);
    let started = Instant::now();
    let result = go_with(
        &rt,
        RunRequest {
            program: "await tools.slow(30000); return 1;".into(),
            bindings: vec![tools()],
            timeout: Timeout::Ms(300),
            ..RunRequest::default()
        },
    )
    .await;
    assert_eq!(error_of(&result).kind, FailureKind::Timeout, "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_deadline_runs_to_the_end_and_the_cancel_token_stops_a_program_mid_loop() {
    let rt = runtime(ExecutionMode::PerCall);
    let free = go_with(
        &rt,
        RunRequest {
            program: "return 'done';".into(),
            timeout: Timeout::None,
            ..RunRequest::default()
        },
    )
    .await;
    assert_eq!(free.value, Some(json!("done")));
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stopper.cancel();
    });
    let result = go_with(
        &rt,
        RunRequest {
            program: "while (true) {}".into(),
            timeout: Timeout::None,
            cancel: Some(cancel),
            ..RunRequest::default()
        },
    )
    .await;
    assert_eq!(error_of(&result).kind, FailureKind::Abort, "{result:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn too_much_output_is_an_output_limit_failure_that_keeps_what_fit() {
    let rt = Runtime::new(Config {
        limits: Limits {
            max_output_bytes: 200,
            ..Config::default().limits
        },
        ..Config::default()
    })
    .expect("runtime");
    let result = go(&rt, "console.log('first line'); console.log('x'.repeat(5000)); console.log('never seen'); return 1;").await;
    assert_eq!(
        error_of(&result).kind,
        FailureKind::OutputLimit,
        "{result:?}"
    );
    assert_eq!(result.logs[0], "first line");
    assert!(!result.logs.iter().any(|l| l == "never seen"));
    let total: usize = serde_json::to_string(&result.logs).expect("json").len()
        + error_of(&result).message.len()
        + 2;
    assert!(total <= 200, "{total}");
    let big_value = go(&rt, "return 'y'.repeat(5000);").await;
    assert_eq!(error_of(&big_value).kind, FailureKind::OutputLimit);
    assert!(big_value.value.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completion_that_is_not_lossless_json_is_refused_and_says_which_part() {
    let rt = runtime(ExecutionMode::PerCall);
    let cases = [
        ("return { a: undefined };", "undefined"),
        ("return NaN;", "finite"),
        ("return Infinity;", "finite"),
        ("return -0;", "negative zero"),
        ("return () => 1;", "function"),
        ("return Symbol('s');", "symbol"),
        ("return 10n;", "bigint"),
        ("const o = {}; o.self = o; return o;", "circular"),
        ("return new Date(0);", "not a plain object"),
        (
            "class A { constructor() { this.x = 1; } } return new A();",
            "not a plain object",
        ),
        ("const a = [1]; a[3] = 4; return a;", "sparse"),
        ("return { get g() { return 1; } };", "accessor"),
        ("return { [Symbol('k')]: 1 };", "symbol-keyed"),
        (
            "const o = {}; Object.defineProperty(o, 'hidden', { value: 1 }); return o;",
            "not enumerable",
        ),
        ("return new Map();", "not a plain object"),
    ];
    for (program, expected) in cases {
        let result = go(&rt, program).await;
        let failure = error_of(&result);
        assert_eq!(
            failure.kind,
            FailureKind::InvalidOutput,
            "{program}: {result:?}"
        );
        assert!(
            failure.message.contains(expected),
            "{program}: {}",
            failure.message
        );
    }
    let fine = go(
        &rt,
        "return { a: [1, 2.5, 'é', null, true], b: { c: {} }, d: Object.create(null) };",
    )
    .await;
    assert_eq!(fine.error, None, "{fine:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_nested_too_deep_is_refused() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(
        &rt,
        "let v = []; for (let i = 0; i < 1500; i++) v = [v]; return v;",
    )
    .await;
    assert_eq!(
        error_of(&result).kind,
        FailureKind::InvalidOutput,
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn arguments_that_are_not_json_are_refused_before_they_leave_the_program() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(
        &rt,
        "try { await tools.echo({ f() {} }); } catch (e) { return e.message; }",
    )
    .await;
    assert!(
        result
            .value
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(|m| m.contains("function")),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_calls_at_once_or_arguments_that_are_too_big_break_the_protocol() {
    let rt = Runtime::new(Config {
        limits: Limits {
            max_pending_calls: 3,
            max_message_bytes: 100,
            ..Config::default().limits
        },
        ..Config::default()
    })
    .expect("runtime");
    let many = go(
        &rt,
        "await Promise.all([1, 2, 3, 4, 5].map((n) => tools.echo(n)));",
    )
    .await;
    assert_eq!(error_of(&many).kind, FailureKind::Protocol, "{many:?}");
    let big = go(&rt, "await tools.echo('x'.repeat(1000));").await;
    assert_eq!(error_of(&big).kind, FailureKind::Protocol, "{big:?}");
    let fine = go(
        &rt,
        "return await Promise.all([1, 2, 3].map((n) => tools.echo(n)));",
    )
    .await;
    assert_eq!(fine.value, Some(json!([1, 2, 3])));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_eats_memory_or_stack_fails_instead_of_taking_the_app_down() {
    let rt = runtime(ExecutionMode::PerCall);
    assert_eq!(
        error_of(
            &go(
                &rt,
                "const big = []; while (true) { big.push('x'.repeat(1 << 20)); }"
            )
            .await
        )
        .kind,
        FailureKind::Exception
    );
    assert_eq!(
        error_of(&go(&rt, "function f(n) { return f(n + 1) + 1; } return f(0);").await).kind,
        FailureKind::Exception
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_promise_that_never_settles_is_reported_not_waited_on_forever() {
    let rt = runtime(ExecutionMode::PerCall);
    let result = go(&rt, "await new Promise(() => {}); return 1;").await;
    assert!(
        error_of(&result).message.contains("can never settle"),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn retained_state_keeps_top_level_variables_until_the_owner_is_released() {
    let rt = runtime(ExecutionMode::Persistent);
    let owner = rt.new_owner();
    let request = |program: &str| RunRequest {
        program: program.to_owned(),
        state_owner: Some(owner),
        bindings: vec![tools()],
        ..RunRequest::default()
    };
    assert_eq!(
        go_with(
            &rt,
            request("let count = 1; function bump() { count += 1; return count; } count")
        )
        .await
        .value,
        Some(json!(1))
    );
    assert_eq!(
        go_with(&rt, request("bump(); bump()")).await.value,
        Some(json!(3))
    );
    let again = go_with(&rt, request("let count = 5;")).await;
    assert!(
        error_of(&again).message.contains("count"),
        "a `let` cannot be declared twice: {again:?}"
    );
    assert_eq!(
        go_with(&rt, request("count = 10; count")).await.value,
        Some(json!(10)),
        "but it can be assigned"
    );
    rt.release(owner).await;
    let fresh = go_with(&rt, request("typeof count")).await;
    assert_eq!(
        fresh.value,
        Some(json!("undefined")),
        "released: empty again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn owners_are_isolated_and_runs_without_an_owner_retain_nothing() {
    let rt = runtime(ExecutionMode::Persistent);
    let (a, b) = (rt.new_owner(), rt.new_owner());
    let run = |owner: Option<OwnerId>, program: &str| RunRequest {
        program: program.to_owned(),
        state_owner: owner,
        ..RunRequest::default()
    };
    go_with(&rt, run(Some(a), "var secret = 'a';")).await;
    assert_eq!(
        go_with(&rt, run(Some(b), "typeof secret")).await.value,
        Some(json!("undefined"))
    );
    assert_eq!(
        go_with(&rt, run(Some(a), "secret")).await.value,
        Some(json!("a"))
    );
    go_with(&rt, run(None, "var temp = 1;")).await;
    assert_eq!(
        go_with(&rt, run(None, "typeof temp")).await.value,
        Some(json!("undefined"))
    );
    rt.release(a).await;
    rt.release(b).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn per_call_mode_ignores_the_owner_and_keeps_nothing() {
    let rt = runtime(ExecutionMode::PerCall);
    assert!(!rt.retains_state());
    let owner = rt.new_owner();
    let run = |program: &str| RunRequest {
        program: program.to_owned(),
        state_owner: Some(owner),
        ..RunRequest::default()
    };
    go_with(&rt, run("globalThis.kept = 1;")).await;
    assert_eq!(
        go_with(&rt, run("return typeof globalThis.kept;"))
            .await
            .value,
        Some(json!("undefined"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_stopped_from_outside_clears_the_retained_variables_and_says_so() {
    let rt = runtime(ExecutionMode::Persistent);
    let owner = rt.new_owner();
    let run = |program: &str, timeout: Timeout| RunRequest {
        program: program.to_owned(),
        state_owner: Some(owner),
        timeout,
        ..RunRequest::default()
    };
    go_with(&rt, run("var kept = 1;", Timeout::Default)).await;
    let stopped = go_with(&rt, run("while (true) {}", Timeout::Ms(200))).await;
    assert_eq!(error_of(&stopped).kind, FailureKind::Timeout);
    assert!(
        error_of(&stopped).message.contains("cleared"),
        "{}",
        error_of(&stopped).message
    );
    assert_eq!(
        go_with(&rt, run("typeof kept", Timeout::Default))
            .await
            .value,
        Some(json!("undefined"))
    );
    // An exception in a program is not an outside stop: what was set before it stays.
    go_with(
        &rt,
        run("var survives = 1; throw new Error('x');", Timeout::Default),
    )
    .await;
    assert_eq!(
        go_with(&rt, run("survives", Timeout::Default)).await.value,
        Some(json!(1))
    );
    rt.release(owner).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retained_function_uses_the_bindings_of_the_run_that_calls_it() {
    let rt = runtime(ExecutionMode::Persistent);
    let owner = rt.new_owner();
    let with = |program: &str, answer: &'static str| RunRequest {
        program: program.to_owned(),
        state_owner: Some(owner),
        bindings: vec![namespace(
            "host",
            vec![("name", function(move |_| async move { Ok(json!(answer)) }))],
            None,
        )],
        ..RunRequest::default()
    };
    go_with(
        &rt,
        with(
            "async function ask() { return await host.name(); } 0",
            "first",
        ),
    )
    .await;
    // A promise as the last value is waited for, and the retained function resolves through this run's bindings.
    let second = go_with(&rt, with("ask()", "second")).await;
    assert_eq!(second.value, Some(json!("second")), "{second:?}");
    rt.release(owner).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_that_nothing_awaits_are_not_answered_after_the_program_has_ended() {
    let rt = runtime(ExecutionMode::Persistent);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let host = namespace(
        "host",
        vec![(
            "hit",
            function(move |_| {
                let counted = counted.clone();
                async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(true))
                }
            }),
        )],
        None,
    );
    let result = go_with(
        &rt,
        RunRequest {
            program: "host.hit(); ({ ok: 1 })".into(),
            bindings: vec![host.clone()],
            ..RunRequest::default()
        },
    )
    .await;
    assert_eq!(result.value, Some(json!({ "ok": 1 })), "{result:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a program that ends without awaiting a call does not wait for it"
    );
    let awaited = go_with(
        &rt,
        RunRequest {
            program: "Promise.all([host.hit(), host.hit()]).then((all) => all.length)".into(),
            bindings: vec![host],
            ..RunRequest::default()
        },
    )
    .await;
    assert_eq!(
        (awaited.value, calls.load(Ordering::SeqCst)),
        (Some(json!(2)), 2)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_runs_of_one_owner_at_once_are_refused_as_misuse() {
    let rt = Arc::new(runtime(ExecutionMode::Persistent));
    let owner = rt.new_owner();
    let slow = RunRequest {
        program: "await_me = 1; while (true) {}".into(),
        state_owner: Some(owner),
        timeout: Timeout::Ms(600),
        ..RunRequest::default()
    };
    let spec = rt.resolve(slow).expect("resolves");
    let first = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.run(spec).await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let second = rt
        .resolve(RunRequest {
            program: "1".into(),
            state_owner: Some(owner),
            ..RunRequest::default()
        })
        .expect("resolves");
    assert!(
        rt.run(second)
            .await
            .unwrap_err()
            .contains("already running")
    );
    assert_eq!(
        error_of(&first.await.expect("joins").expect("runs")).kind,
        FailureKind::Timeout
    );
    rt.release(owner).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resolving_fills_defaults_caps_deadlines_and_refuses_what_it_cannot_honour() {
    let rt = Runtime::new(Config {
        timeout_ms: 1000,
        max_timeout_ms: 5000,
        ..Config::default()
    })
    .expect("runtime");
    let spec = |timeout: Timeout| {
        rt.resolve(RunRequest {
            program: "1".into(),
            timeout,
            ..RunRequest::default()
        })
    };
    assert_eq!(spec(Timeout::Default).expect("ok").timeout_ms, Some(1000));
    assert_eq!(spec(Timeout::Ms(300)).expect("ok").timeout_ms, Some(300));
    assert_eq!(
        spec(Timeout::Ms(999_999)).expect("ok").timeout_ms,
        Some(5000),
        "capped"
    );
    assert_eq!(spec(Timeout::None).expect("ok").timeout_ms, None);
    assert!(spec(Timeout::Ms(0)).is_err());
    assert!(
        rt.resolve(RunRequest {
            program: "1".into(),
            cwd: Some("relative/dir".into()),
            ..RunRequest::default()
        })
        .is_err()
    );
    assert!(
        rt.resolve(RunRequest {
            program: "1".into(),
            bindings: vec![namespace("console", vec![], None)],
            ..RunRequest::default()
        })
        .is_err()
    );
    assert!(
        rt.resolve(RunRequest {
            program: "1".into(),
            bindings: vec![
                namespace("tools", vec![], None),
                namespace("tools", vec![], None)
            ],
            ..RunRequest::default()
        })
        .is_err()
    );
    assert_eq!(rt.timeout(), (1000, 5000));
}

#[test]
fn settings_must_be_positive_and_the_default_deadline_may_not_exceed_the_cap() {
    assert!(Config::default().validate().is_ok());
    assert!(
        Config {
            timeout_ms: 0,
            ..Config::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            timeout_ms: 10,
            max_timeout_ms: 5,
            ..Config::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            limits: Limits {
                max_pending_calls: 0,
                ..Config::default().limits
            },
            ..Config::default()
        }
        .validate()
        .is_err()
    );
    let rt = Runtime::new(Config::default()).expect("runtime");
    assert_eq!(
        (rt.language(), rt.isolation(), rt.program_format()),
        ("javascript", "thread", ProgramFormat::FunctionBody)
    );
    assert!(rt.execution_instructions().contains("async function"));
    let persistent = Runtime::new(Config {
        mode: ExecutionMode::Persistent,
        ..Config::default()
    })
    .expect("runtime");
    assert!(
        persistent.retains_state()
            && persistent
                .execution_instructions()
                .contains("stay for the next run")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn after_disposal_nothing_runs_and_every_environment_is_gone() {
    let rt = runtime(ExecutionMode::Persistent);
    let owner = rt.new_owner();
    go_with(
        &rt,
        RunRequest {
            program: "var x = 1;".into(),
            state_owner: Some(owner),
            ..RunRequest::default()
        },
    )
    .await;
    rt.dispose().await;
    assert!(
        rt.resolve(RunRequest {
            program: "1".into(),
            ..RunRequest::default()
        })
        .is_err()
    );
    let spec = RunSpec {
        program: "1".into(),
        state_owner: None,
        bindings: vec![],
        cwd: std::env::temp_dir(),
        timeout_ms: Some(1000),
        cancel: None,
    };
    assert!(rt.run(spec).await.is_err());
}
