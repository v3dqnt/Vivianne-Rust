//! Real processes: the shell of the platform, so the tests run on Windows and Unix alike.

use std::time::{Duration, Instant};

use super::*;

fn shell(script: &str) -> Spec {
    let argv: Vec<&str> = if cfg!(windows) {
        vec![
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]
    } else {
        vec!["sh", "-c", script]
    };
    let mut spec = Spec::new(argv, std::env::temp_dir());
    spec.stdout = Output::Collect {
        max_bytes: 4096,
        spill: None,
    };
    spec.stderr = Output::Collect {
        max_bytes: 4096,
        spill: None,
    };
    spec.grace = Duration::from_millis(300);
    spec
}

fn sleep_script(seconds: u32) -> String {
    if cfg!(windows) {
        format!("Start-Sleep -Seconds {seconds}")
    } else {
        format!("sleep {seconds}")
    }
}

#[tokio::test]
async fn output_and_exit_code_come_back() {
    let script = if cfg!(windows) {
        "Write-Output hello; exit 3"
    } else {
        "echo hello; exit 3"
    };
    let mut handle = spawn(shell(script)).expect("spawns");
    let outcome = handle.done().await;
    assert_eq!(outcome.exit_code, Some(3));
    assert_eq!(
        handle.collected().0.expect("collected").text.trim(),
        "hello"
    );
}

#[tokio::test]
async fn input_data_is_written_and_closed() {
    let script = if cfg!(windows) {
        "$input | ForEach-Object { \"got $_\" }"
    } else {
        "read x; echo \"got $x\""
    };
    let mut spec = shell(script);
    spec.stdin = Input::Data(b"marker\n".to_vec());
    let mut handle = spawn(spec).expect("spawns");
    handle.done().await;
    assert!(
        handle
            .collected()
            .0
            .expect("collected")
            .text
            .contains("got marker")
    );
}

#[tokio::test]
async fn the_child_sees_the_scrubbed_environment_and_the_callers_entries() {
    let script = if cfg!(windows) {
        "Write-Output \"[$env:VIV_SECRET_PROBE][$env:EXTRA]\""
    } else {
        "echo \"[$VIV_SECRET_PROBE][$EXTRA]\""
    };
    let mut spec = shell(script);
    spec.env.insert("EXTRA".into(), Some("yes".into()));
    let mut handle = spawn(spec).expect("spawns");
    handle.done().await;
    assert!(
        handle
            .collected()
            .0
            .expect("collected")
            .text
            .contains("[][yes]")
    );
}

#[tokio::test]
async fn terminate_ends_a_running_process_quickly() {
    let mut handle = spawn(shell(&sleep_script(60))).expect("spawns");
    assert!(!handle.wait_for_exit(Duration::from_millis(200)).await);
    let started = Instant::now();
    handle.terminate();
    assert!(
        handle.wait_for_exit(Duration::from_secs(10)).await,
        "it ends"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_ne!(handle.outcome().expect("ended").exit_code, Some(0));
}

/// A parent that starts a grandchild which writes `marker` after three seconds, then waits `parent_waits`.
fn with_grandchild(marker: &std::path::Path, parent_waits: u32) -> Spec {
    let path = marker.to_string_lossy();
    let script = if cfg!(windows) {
        format!(
            "Start-Process -NoNewWindow powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 3; Set-Content -Path ''{path}'' -Value x'; Start-Sleep {parent_waits}"
        )
    } else {
        format!("(sleep 3; echo x > '{path}') & sleep {parent_waits}")
    };
    shell(&script)
}

#[tokio::test]
async fn ending_it_ends_what_it_started_too() {
    let dir = std::env::temp_dir().join(format!("viv-tree-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    // Control: left alone, the grandchild does write. Without this the check below could pass for nothing.
    let control = dir.join("left-alone");
    let mut free = spawn(with_grandchild(&control, 9)).expect("spawns");
    free.done().await;
    assert!(
        control.exists(),
        "the grandchild writes when nothing stops it"
    );

    let marker = dir.join("grandchild-lived");
    let mut handle = spawn(with_grandchild(&marker, 60)).expect("spawns");
    // Let the grandchild start (PowerShell is slow to come up).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    handle.terminate();
    assert!(handle.wait_for_exit(Duration::from_secs(10)).await);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!marker.exists(), "the grandchild was ended with its parent");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_cancelled_token_terminates_and_a_dropped_handle_does_too() {
    let cancel = CancellationToken::new();
    let mut spec = shell(&sleep_script(60));
    spec.cancel = Some(cancel.clone());
    let mut handle = spawn(spec).expect("spawns");
    cancel.cancel();
    assert!(handle.wait_for_exit(Duration::from_secs(10)).await);
}

#[tokio::test]
async fn what_cannot_start_says_why() {
    let none: [&str; 0] = [];
    assert!(matches!(
        spawn(Spec::new(none, std::env::temp_dir())),
        Err(SpawnError::Empty)
    ));
    let missing = Spec::new(["viv-no-such-program"], std::env::temp_dir());
    assert!(matches!(spawn(missing), Err(SpawnError::NotFound(p)) if p == "viv-no-such-program"));
    let folder = Spec::new(["sh"], std::env::temp_dir().join("viv-no-such-folder"));
    assert!(matches!(spawn(folder), Err(SpawnError::NotAFolder(_))));
}
