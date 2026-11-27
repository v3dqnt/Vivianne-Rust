//! Against a real shell on a real pseudo terminal.

use serde_json::{Value, json};

use super::*;
use crate::builtin::Tool;

fn terminals() -> Terminals {
    Terminals::new(
        &std::env::temp_dir(),
        Confinement::default(),
        Jobs::default(),
    )
}

async fn call(terminals: &Terminals, kind: Kind, args: Value) -> crate::builtin::ToolResult {
    TerminalTool(kind, terminals.clone())
        .execute(&args, "")
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn state_survives_between_sends_and_closing_ends_the_session() {
    let terminals = terminals();
    let opened = call(
        &terminals,
        Kind::Open,
        json!({"type": "shell", "name": "main"}),
    )
    .await;
    assert!(
        opened
            .content
            .starts_with("started terminal session term-1 (main)"),
        "{}",
        opened.content
    );

    let set = if cfg!(windows) {
        "$v = 'kept' + 'over'"
    } else {
        "v=kept''over"
    };
    call(
        &terminals,
        Kind::Send,
        json!({"sessionId": "term-1", "text": set}),
    )
    .await;
    let said = call(
        &terminals,
        Kind::Send,
        json!({"sessionId": "term-1", "text": "echo $v"}),
    )
    .await;
    assert!(said.content.contains("keptover"), "{}", said.content);
    assert!(
        said.content
            .ends_with("[wait: inferred_idle]\n[session: running]"),
        "{}",
        said.content
    );

    let page = call(
        &terminals,
        Kind::Read,
        json!({"sessionId": "term-1", "count": 3}),
    )
    .await;
    assert!(
        page.content.contains("keptover") && page.content.contains("[lines "),
        "{}",
        page.content
    );

    let listed = call(&terminals, Kind::List, json!({})).await;
    assert_eq!(listed.content, "term-1 (main) [shell] running");
    let closed = call(&terminals, Kind::Close, json!({"sessionId": "term-1"})).await;
    assert_eq!(closed.content, "closed terminal session term-1");
    assert_eq!(
        call(&terminals, Kind::List, json!({})).await.content,
        "(no terminal sessions)"
    );
    assert!(
        call(&terminals, Kind::Read, json!({"sessionId": "term-1"}))
            .await
            .is_error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exit_is_reported_and_nothing_more_can_be_sent() {
    let terminals = terminals();
    call(&terminals, Kind::Open, json!({"type": "shell"})).await;
    let out = call(
        &terminals,
        Kind::Send,
        json!({"sessionId": "term-1", "text": "exit 3"}),
    )
    .await;
    assert!(
        out.content.contains("[wait: session_exit]"),
        "{}",
        out.content
    );
    assert!(out.content.contains("exited code="), "{}", out.content);
    assert!(
        call(
            &terminals,
            Kind::Send,
            json!({"sessionId": "term-1", "text": "x"})
        )
        .await
        .is_error
    );
}

#[tokio::test]
async fn what_cannot_be_asked_for_is_refused() {
    let terminals = terminals();
    for args in [
        json!({"type": "gdb"}),
        json!({"type": "shell", "name": ""}),
        json!({"type": "shell", "cwd": "../.."}),
    ] {
        assert!(
            call(&terminals, Kind::Open, args.clone()).await.is_error,
            "{args}"
        );
    }
    assert!(
        call(
            &terminals,
            Kind::Signal,
            json!({"sessionId": "term-9", "signal": "SIGINT"})
        )
        .await
        .is_error
    );
}

#[tokio::test]
async fn a_mode_that_cannot_be_enforced_opens_no_terminal() {
    // This test program is no sandbox runner, so on Windows a confined mode has no backend here.
    if cfg!(windows) {
        let confinement = Confinement::default();
        confinement.set(Mode::ReadOnly);
        let terminals = Terminals::new(&std::env::temp_dir(), confinement, Jobs::default());
        let out = call(&terminals, Kind::Open, json!({"type": "shell"})).await;
        assert!(
            out.is_error && out.content.contains("refusing"),
            "{}",
            out.content
        );
    }
}

/// Run alone, since naming a runner changes the whole test program:
/// `VIV_TEST_RUNNER=<path to viv.exe> cargo test -p vivianne-tools confined_terminal -- --ignored`
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a built runner"]
async fn confined_terminal_runs_and_cannot_write() {
    let runner = std::env::var_os("VIV_TEST_RUNNER").expect("VIV_TEST_RUNNER");
    vivianne_sandbox::runner::use_helper(runner.into());
    let dir = std::env::temp_dir().join(format!("viv-term-ro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let confinement = Confinement::default();
    confinement.set(Mode::ReadOnly);
    let terminals = Terminals::new(&dir, confinement, Jobs::default());
    let opened = call(&terminals, Kind::Open, json!({"type": "shell"})).await;
    assert!(!opened.is_error, "{}", opened.content);
    let out = call(
        &terminals,
        Kind::Send,
        json!({"sessionId": "term-1", "text": "'al' + 'ive'; Set-Content x.txt x"}),
    )
    .await;
    assert!(
        out.content.contains("alive") && out.content.contains("denied"),
        "{}",
        out.content
    );
    assert!(!dir.join("x.txt").exists());
    call(&terminals, Kind::Close, json!({"sessionId": "term-1"})).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let _ = std::fs::remove_dir_all(&dir);
}
