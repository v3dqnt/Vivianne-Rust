// Copied from Vivianne's original tools (0901 proc.rs and the process-tree helpers of agent/runtime/streaming.rs).
// Kept as close to the original as possible; do not restyle.
#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, clippy::todo, clippy::dbg_macro, unsafe_code, missing_debug_implementations)]

//! Starting other programs without a console window flashing up.
//!
//! On Windows a console program started from a windowed app gets a console
//! of its own unless told otherwise, so every `git status` behind a panel
//! blinked a terminal onto the screen. Everything that runs a program in the
//! background starts it through here. Programs meant to be seen (a terminal
//! the user asked for, an app being opened) use `Command::new` directly.

use std::ffi::OsStr;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `tokio` command that opens no window.
pub fn quiet(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// A blocking command that opens no window.
pub fn quiet_std(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
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
        let _ = quiet_std("taskkill").args(["/PID", &pid.to_string(), "/F", "/T"]).spawn();
        #[cfg(not(windows))]
        let _ = quiet_std("kill").args(["-KILL", &format!("-{pid}")]).spawn();
    }
}
