//! The Windows runner against real children. Every folder that gets a grant is a fresh one inside the temporary
//! folder, never the temporary folder itself: a grant relabels its whole tree for good.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use vivianne_sandbox::confine::{self, Backend, Confined};
use vivianne_sandbox::policy::{Mode, Policy};
use vivianne_sandbox::runner;
use vivianne_sandbox::spawn::{self, Output, Spec};

/// A fresh scratch folder holding `ws` (the workspace) and `outside`, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viv-sbx-test-{name}-{nanos:x}"));
        assert_ne!(root, std::env::temp_dir());
        std::fs::create_dir_all(root.join("ws")).expect("ws");
        std::fs::create_dir_all(root.join("outside")).expect("outside");
        Self(root)
    }

    fn ws(&self) -> PathBuf {
        self.0.join("ws")
    }

    fn outside(&self) -> PathBuf {
        self.0.join("outside")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let removed = std::fs::remove_dir_all(&self.0);
        if !std::thread::panicking() {
            removed.expect("a granted folder can still be removed by its owner");
        }
    }
}

struct Ran {
    code: Option<i32>,
    out: String,
    err: String,
}

async fn run(argv: Vec<String>, cwd: &Path) -> Ran {
    let mut spec = Spec::new(argv, cwd);
    let keep = || Output::Collect {
        max_bytes: 64 * 1024,
        spill: None,
    };
    spec.stdout = keep();
    spec.stderr = keep();
    let mut handle = spawn::spawn(spec).expect("spawn");
    assert!(
        handle.wait_for_exit(Duration::from_secs(120)).await,
        "ended"
    );
    let code = handle.done().await.exit_code;
    let (out, err) = handle.collected();
    Ran {
        code,
        out: out.map(|c| c.text).unwrap_or_default(),
        err: err.map(|c| c.text).unwrap_or_default(),
    }
}

fn confined(argv: &[&str], mode: Mode, ws: &Path) -> Confined {
    runner::use_helper(PathBuf::from(env!("CARGO_BIN_EXE_viv-sandbox-run")));
    let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
    let policy = Policy::new(mode, ws, &[]).expect("policy");
    confine::confine_with(Backend::WindowsAcl, &argv, &policy).expect("confined")
}

fn powershell(script: &str) -> [&str; 5] {
    [
        "powershell",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]
}

/// A script that writes `x` into each file and reports which writes worked.
fn writes(files: &[&Path]) -> String {
    files
        .iter()
        .map(|file| {
            let file = file.display();
            format!("try {{ Set-Content -LiteralPath '{file}' -Value x -ErrorAction Stop; 'wrote {file}' }} catch {{ 'denied {file}' }}; ")
        })
        .collect()
}

#[tokio::test]
async fn workspace_write_writes_inside_and_nowhere_else() {
    let scratch = Scratch::new("write");
    let inside = scratch.ws().join("in.txt");
    let outside = scratch.outside().join("out.txt");
    let script = format!(
        "{} Set-Content -LiteralPath \"$env:TEMP\\t.txt\" -Value x; \"temp=$env:TEMP\"",
        writes(&[&inside, &outside])
    );
    let ran = run(
        confined(&powershell(&script), Mode::WorkspaceWrite, &scratch.ws()).argv,
        &scratch.ws(),
    )
    .await;
    assert_eq!(ran.code, Some(0), "{}\n{}", ran.out, ran.err);
    assert!(inside.exists(), "{}\n{}", ran.out, ran.err);
    assert!(!outside.exists(), "{}", ran.out);
    assert!(
        ran.out.contains(&format!("denied {}", outside.display())),
        "{}",
        ran.out
    );
    // The command had its own temporary folder, which is gone with it.
    let temp = ran
        .out
        .lines()
        .find_map(|l| l.trim().strip_prefix("temp="))
        .expect("temp");
    assert!(
        temp.contains("viv-sbx-") && !Path::new(temp).exists(),
        "{temp}"
    );

    // Deleting outside is refused as well.
    let victim = scratch.outside().join("keep.txt");
    std::fs::write(&victim, "keep").expect("victim");
    let script = format!("Remove-Item -LiteralPath '{}' -Force", victim.display());
    let ran = run(
        confined(&powershell(&script), Mode::WorkspaceWrite, &scratch.ws()).argv,
        &scratch.ws(),
    )
    .await;
    assert!(victim.exists(), "{}\n{}", ran.out, ran.err);

    // A grant already made does not open the folder to a later read-only command.
    let again = scratch.ws().join("again.txt");
    let ran = run(
        confined(
            &powershell(&writes(&[&again])),
            Mode::ReadOnly,
            &scratch.ws(),
        )
        .argv,
        &scratch.ws(),
    )
    .await;
    assert!(!again.exists(), "{}\n{}", ran.out, ran.err);
    assert!(ran.out.contains("denied"), "{}\n{}", ran.out, ran.err);
}

#[tokio::test]
async fn read_only_reads_but_writes_nothing_while_an_unconfined_run_does() {
    let scratch = Scratch::new("read");
    let file = scratch.ws().join("a.txt");
    std::fs::write(scratch.ws().join("b.txt"), "readable").expect("b");
    let script = format!("Get-Content b.txt; {}", writes(&[&file]));
    let confined = confined(&powershell(&script), Mode::ReadOnly, &scratch.ws());
    let ran = run(confined.argv, &scratch.ws()).await;
    assert!(
        ran.out.contains("readable") && ran.out.contains("denied"),
        "{}\n{}",
        ran.out,
        ran.err
    );
    assert!(!file.exists());

    let control = run(
        powershell(&script).map(str::to_owned).to_vec(),
        &scratch.ws(),
    )
    .await;
    assert!(
        file.exists(),
        "the control writes: {}\n{}",
        control.out,
        control.err
    );
}

#[tokio::test]
async fn a_grandchild_is_held_and_the_exit_code_and_arguments_pass_through() {
    let scratch = Scratch::new("child");
    let outside = scratch.outside().join("g.txt");
    // `cmd` starts PowerShell, which does the writing: no quotes, which `cmd` would read differently.
    let line = format!(
        "powershell -NoProfile -Command Set-Content -LiteralPath {} -Value x & exit /b 7",
        outside.display()
    );
    let confined = confined(
        &["cmd", "/d", "/c", &line],
        Mode::WorkspaceWrite,
        &scratch.ws(),
    );
    let ran = run(confined.argv.clone(), &scratch.ws()).await;
    assert_eq!(ran.code, Some(7), "{}\n{}", ran.out, ran.err);
    assert!(!outside.exists());
    assert!(
        confine::denied(ran.code, &ran.err, confined.denial_signatures),
        "{}",
        ran.err
    );
}

#[tokio::test]
async fn a_folder_that_may_not_be_granted_runs_nothing() {
    let scratch = Scratch::new("refuse");
    let marker = scratch.ws().join("ran.txt");
    let script = writes(&[&marker]);
    // The temporary folder itself holds everyone's temporary files.
    let confined = confined(
        &powershell(&script),
        Mode::WorkspaceWrite,
        &std::env::temp_dir(),
    );
    let ran = run(confined.argv.clone(), &scratch.ws()).await;
    assert_eq!(
        ran.code,
        Some(runner::FAILURE_EXIT),
        "{}\n{}",
        ran.out,
        ran.err
    );
    assert!(
        ran.err.contains("cannot be a writable folder"),
        "{}",
        ran.err
    );
    assert!(!marker.exists());
}
