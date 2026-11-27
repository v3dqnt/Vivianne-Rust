//! Turning a command into a confined one. Linux uses `bwrap`, macOS `sandbox-exec` (Seatbelt), Windows a restricted
//! token through this program's own runner (`runner.rs`). Where none is usable the mode is refused rather than run
//! unconfined: a command never runs with more than the mode says.

use std::path::PathBuf;

use crate::policy::{Mode, Policy, PolicyError};

/// How completely a backend enforces the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enforcement {
    Full,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Bwrap,
    Seatbelt,
    /// Writes and deletes only; reads and the network stay open.
    WindowsAcl,
}

/// A command rewritten to run confined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confined {
    pub argv: Vec<String>,
    pub enforcement: Enforcement,
    /// Lines in standard error that mean the policy denied a file access.
    pub denial_signatures: &'static [&'static str],
    /// Lines that mean the runner itself failed, not the command.
    pub failure_signatures: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfineError {
    #[error(
        "sandbox mode \"{0}\" is requested but no sandbox backend is usable on this host; refusing to run the command unconfined. Install bubblewrap (Linux) or check sandbox-exec (macOS); on Windows this program cannot act as the runner. Otherwise choose danger-full-access."
    )]
    Unavailable(&'static str),
    #[error("{0}")]
    Policy(#[from] PolicyError),
    #[error("danger-full-access is not confined; run the command as it is")]
    NotConfinedMode,
}

fn text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `bwrap` arguments: the whole system read-only, the writable folders bound over it, a private `/tmp`.
pub fn bwrap_args(policy: &Policy) -> Result<Vec<String>, PolicyError> {
    let mut args: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--unshare-pid",
        "--proc",
        "/proc",
        "--die-with-parent",
    ]
    .map(str::to_owned)
    .to_vec();
    if policy.mode == Mode::WorkspaceWrite {
        args.extend(["--tmpfs".to_owned(), "/tmp".to_owned()]);
        for root in policy.workspace_roots()? {
            args.extend(["--bind".to_owned(), text(&root), text(&root)]);
        }
    }
    Ok(args)
}

/// A path as a Seatbelt string.
fn quoted(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `sandbox-exec` arguments: everything allowed but writing, then writing allowed under the writable folders.
pub fn seatbelt_args(policy: &Policy) -> Result<Vec<String>, PolicyError> {
    let mut forms = vec![
        "(version 1)".to_owned(),
        "(allow default)".to_owned(),
        "(deny file-write*)".to_owned(),
        format!("(allow file-write* (literal {}))", quoted("/dev/null")),
    ];
    let roots: Vec<PathBuf> = policy.writable_roots()?;
    if !roots.is_empty() {
        let list: Vec<String> = roots
            .iter()
            .map(|r| format!("(subpath {})", quoted(&text(r))))
            .collect();
        forms.push(format!("(allow file-write* {})", list.join(" ")));
    }
    Ok(vec!["-p".to_owned(), forms.join(" ")])
}

/// `argv` as it must be run under `backend` to hold `policy`.
pub fn confine_with(
    backend: Backend,
    argv: &[String],
    policy: &Policy,
) -> Result<Confined, ConfineError> {
    if policy.mode == Mode::DangerFullAccess {
        return Err(ConfineError::NotConfinedMode);
    }
    if backend == Backend::WindowsAcl {
        let helper =
            crate::runner::helper().ok_or(ConfineError::Unavailable(policy.mode.as_str()))?;
        return windows_acl(helper, argv, policy);
    }
    let (program, profile, denial_signatures, failure_signatures): (
        _,
        _,
        &'static [&'static str],
        &'static [&'static str],
    ) = match backend {
        Backend::Bwrap => (
            "bwrap",
            bwrap_args(policy)?,
            &["read-only file system"],
            &["bwrap: "],
        ),
        Backend::WindowsAcl => return Err(ConfineError::Unavailable(policy.mode.as_str())),
        Backend::Seatbelt => (
            "sandbox-exec",
            seatbelt_args(policy)?,
            &["operation not permitted"],
            &["sandbox-exec: "],
        ),
    };
    let mut out = vec![program.to_owned()];
    out.extend(profile);
    out.push("--".to_owned());
    out.extend(argv.iter().cloned());
    Ok(Confined {
        argv: out,
        enforcement: Enforcement::Full,
        denial_signatures,
        failure_signatures,
    })
}

/// `argv` run through the program at `helper` acting as the Windows runner.
fn windows_acl(
    helper: &std::path::Path,
    argv: &[String],
    policy: &Policy,
) -> Result<Confined, ConfineError> {
    let mut out = vec![
        text(helper),
        crate::runner::ARGUMENT.to_owned(),
        "--mode".to_owned(),
        policy.mode.as_str().to_owned(),
    ];
    for root in policy.workspace_roots()? {
        out.extend(["--writable".to_owned(), text(&root)]);
    }
    out.push("--".to_owned());
    out.extend(argv.iter().cloned());
    Ok(Confined {
        argv: out,
        enforcement: Enforcement::Partial,
        denial_signatures: &[
            "access is denied",
            "is denied.",
            "permission denied",
            "viv-sandbox-run: ",
        ],
        failure_signatures: &["viv-sandbox-run: "],
    })
}

/// The backend this machine can use: the platform's runner, once it has actually run a trivial command confined.
#[must_use]
pub fn usable_backend() -> Option<Backend> {
    if cfg!(windows) {
        // Nothing to probe: a runner that cannot confine says so and runs nothing.
        return crate::runner::helper().map(|_| Backend::WindowsAcl);
    }
    let (backend, program) = if cfg!(target_os = "linux") {
        (Backend::Bwrap, "bwrap")
    } else if cfg!(target_os = "macos") {
        (Backend::Seatbelt, "sandbox-exec")
    } else {
        return None;
    };
    let root = Policy::new(Mode::ReadOnly, std::path::Path::new("/"), &[]).ok()?;
    let probe = confine_with(backend, &["true".to_owned()], &root).ok()?;
    let ran = std::process::Command::new(program)
        .args(&probe.argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    ran.is_ok_and(|s| s.success()).then_some(backend)
}

/// Confines `argv` with this machine's backend, or says that it cannot.
pub fn confine(argv: &[String], policy: &Policy) -> Result<Confined, ConfineError> {
    if policy.mode == Mode::DangerFullAccess {
        return Err(ConfineError::NotConfinedMode);
    }
    let backend = usable_backend().ok_or(ConfineError::Unavailable(policy.mode.as_str()))?;
    confine_with(backend, argv, policy)
}

/// The line of standard error showing the runner itself failed, if the command failed that way.
#[must_use]
pub fn runner_failure<'a>(
    exit_code: Option<i32>,
    stderr: &'a str,
    signatures: &[&str],
) -> Option<&'a str> {
    if exit_code.is_none_or(|c| c == 0) {
        return None;
    }
    let wanted: Vec<String> = signatures
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    stderr.lines().find(|line| {
        let low = line.to_lowercase();
        wanted.iter().any(|s| low.contains(s))
    })
}

/// Whether a failed command's error output shows the policy denied it.
#[must_use]
pub fn denied(exit_code: Option<i32>, stderr: &str, signatures: &[&str]) -> bool {
    runner_failure(exit_code, stderr, signatures).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: Mode) -> Policy {
        let ws = std::fs::canonicalize(std::env::temp_dir()).expect("temp");
        Policy::new(mode, &ws, &[]).expect("policy")
    }

    fn argv() -> Vec<String> {
        vec!["sh".into(), "-c".into(), "echo".into()]
    }

    #[test]
    fn bwrap_is_read_only_and_binds_only_what_the_mode_opens() {
        let read =
            confine_with(Backend::Bwrap, &argv(), &policy(Mode::ReadOnly)).expect("confined");
        assert_eq!(read.argv[0], "bwrap");
        assert!(!read.argv.iter().any(|a| a == "--bind" || a == "--tmpfs"));
        assert!(
            read.argv
                .ends_with(&["--".into(), "sh".into(), "-c".into(), "echo".into()])
        );
        let write =
            confine_with(Backend::Bwrap, &argv(), &policy(Mode::WorkspaceWrite)).expect("confined");
        assert!(write.argv.iter().any(|a| a == "--bind"));
    }

    #[test]
    fn a_seatbelt_path_cannot_end_its_own_string() {
        assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
        let read = seatbelt_args(&policy(Mode::ReadOnly)).expect("args");
        assert!(read[1].contains("(deny file-write*)") && !read[1].contains("subpath"));
        let write = seatbelt_args(&policy(Mode::WorkspaceWrite)).expect("args");
        assert!(write[1].contains("(allow file-write* (subpath "));
    }

    #[test]
    fn full_access_is_not_wrapped_and_a_confined_mode_without_a_backend_is_refused() {
        let full = confine_with(Backend::Bwrap, &argv(), &policy(Mode::DangerFullAccess));
        assert_eq!(full, Err(ConfineError::NotConfinedMode));
        if cfg!(windows) {
            let refused = confine(&argv(), &policy(Mode::ReadOnly)).expect_err("refused");
            assert_eq!(refused, ConfineError::Unavailable("read-only"));
            assert!(
                refused
                    .to_string()
                    .contains("refusing to run the command unconfined")
            );
        }
    }

    #[test]
    fn only_a_failed_command_with_the_runners_words_is_a_runner_failure() {
        let sig = ["bwrap: "];
        assert_eq!(
            runner_failure(Some(1), "x\nBWRAP: no perms\n", &sig),
            Some("BWRAP: no perms")
        );
        assert_eq!(runner_failure(Some(0), "bwrap: x", &sig), None);
        assert_eq!(runner_failure(None, "bwrap: x", &sig), None);
        assert_eq!(runner_failure(Some(1), "anything", &[" "]), None);
        assert!(denied(
            Some(1),
            "cp: Read-only file system",
            &["read-only file system"]
        ));
    }
}
