//! The Windows confinement runner is this program itself, started again with [`ARGUMENT`]: it lowers its own token,
//! runs the command under it and mirrors the exit code. A program that wants confined commands on Windows calls
//! [`run_if_asked`] first thing in `main`; one that does not is refused confinement, never run without it.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The first argument that turns a start of the program into a runner.
pub const ARGUMENT: &str = "--viv-sandbox-run";

/// What standard error starts with when the runner failed and the command did not run.
pub const SIGNATURE: &str = "viv-sandbox-run";

/// The runner's exit code for its own failures.
pub const FAILURE_EXIT: i32 = 127;

static HELPER: OnceLock<PathBuf> = OnceLock::new();

/// Names the program that answers [`ARGUMENT`]. The first call decides.
pub fn use_helper(program: PathBuf) {
    let _ = HELPER.set(program);
}

pub(crate) fn helper() -> Option<&'static PathBuf> {
    HELPER.get()
}

/// Runs as the runner and exits if this start asked for it; otherwise notes that this program can be one.
pub fn run_if_asked() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|first| first != ARGUMENT) {
        if let Ok(program) = std::env::current_exe() {
            use_helper(program);
        }
        return;
    }
    #[cfg(windows)]
    let outcome = crate::windows::run(args.collect());
    #[cfg(not(windows))]
    let outcome: Result<i32, String> = Err("only Windows uses this runner".to_owned());
    let code = outcome.unwrap_or_else(|why| {
        eprintln!("{SIGNATURE}: {why}");
        FAILURE_EXIT
    });
    std::process::exit(code);
}
