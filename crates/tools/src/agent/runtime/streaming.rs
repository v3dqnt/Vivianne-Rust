// Copied from Vivianne's original tools (0901 agent/runtime/streaming.rs (process-tree helpers only)).
// Kept as close to the original as possible; do not restyle. Paths such as
// `crate::agent::tools::...` resolve through the re-exports in `lib.rs`.
#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, clippy::todo, clippy::dbg_macro, dead_code, unused, unsafe_code, missing_debug_implementations)]

use std::time::Duration;

/// How long a command gets to exit on SIGTERM before it is killed outright.
const KILL_GRACE: Duration = Duration::from_secs(3);

/// End a process and everything it started (`npm run dev` → `sh` → `node`), not just the process itself, which would leave orphans holding the
/// port. On Windows that is `taskkill /T`; elsewhere the command ran as the leader of its own process group, so the whole group is signalled:
/// first politely, then for good once the grace period is over.
pub(crate) async fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = crate::proc::quiet("taskkill").args(["/PID", &pid.to_string(), "/F", "/T"]).output().await;
    }
    #[cfg(not(windows))]
    {
        let group = format!("-{pid}");
        let _ = crate::proc::quiet("kill").args(["-TERM", &group]).output().await;
        tokio::time::sleep(KILL_GRACE).await;
        let _ = crate::proc::quiet("kill").args(["-KILL", &group]).output().await;
    }
}

/// Put the command in its own process group, so its whole tree can be signalled at once. Call before spawning.
pub(crate) fn own_group(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

/// Ends the command's whole process tree if the call that runs it is dropped (the turn was stopped) before it finished.
pub(crate) struct TreeKillOnDrop {
    pub(crate) pid: Option<u32>,
}

impl Drop for TreeKillOnDrop {
    fn drop(&mut self) {
        let Some(pid) = self.pid.take() else { return };
        #[cfg(windows)]
        let _ = crate::proc::quiet_std("taskkill").args(["/PID", &pid.to_string(), "/F", "/T"]).spawn();
        #[cfg(not(windows))]
        let _ = crate::proc::quiet_std("kill").args(["-KILL", &format!("-{pid}")]).spawn();
    }
}
