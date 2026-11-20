//! The project's own dev server (`npm run dev`, `pnpm dev`, ...): started from here, with the local URL it prints
//! picked out of its output so the app can offer to open it.
//!
//! Ported from the old app's dev-server manager, with the part it left out: a server that exits before printing a
//! URL (a bad script, a missing dependency) is reported as crashed instead of staying "starting" for ever.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Starting,
    Running { url: String },
    Crashed { error: String },
}

struct Instance {
    status: Status,
    child: Option<tokio::process::Child>,
    stop: Arc<AtomicBool>,
}

type Instances = Arc<Mutex<HashMap<String, Instance>>>;

/// Dev servers by id (the project folder, for the app).
#[derive(Default)]
pub struct DevServers {
    instances: Instances,
}

/// The one manager for the app.
pub static DEV_SERVERS: LazyLock<DevServers> = LazyLock::new(DevServers::default);

/// First `http://localhost:PORT`-shaped URL in a line of dev-tool output: Vite (`Local:   http://localhost:5173/`),
/// Next.js, CRA and webpack-dev-server all print one. `0.0.0.0` and `[::1]` are not addresses a browser can open,
/// so both become `127.0.0.1`.
#[must_use]
pub fn detect_url(line: &str) -> Option<String> {
    static URL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(:\d+)?[^\s]*").ok());
    URL.as_ref()?.find(line).map(|m| m.as_str().replace("0.0.0.0", "127.0.0.1").replace("[::1]", "127.0.0.1").trim_end_matches(['/', ')', ',']).to_owned())
}

/// The command that starts the project's dev server, from its `package.json` (`dev`, else `start`) and the package
/// manager its lockfile names. `None` for a folder that is not a Node project or has neither script.
#[must_use]
pub fn detect_command(dir: &Path) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("package.json")).ok()?).ok()?;
    let script = ["dev", "start"].into_iter().find(|s| manifest["scripts"][s].is_string())?;
    let runner = if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun run"
    } else {
        return Some(if script == "start" { "npm start".to_owned() } else { format!("npm run {script}") });
    };
    Some(format!("{runner} {script}"))
}

fn set(instances: &Instances, id: &str, status: Status) {
    if let Some(instance) = instances.lock().unwrap_or_else(PoisonError::into_inner).get_mut(id) {
        instance.status = status;
    }
}

/// Reads output lines until the URL shows up. `watch_exit`: when the stream ends (the process is gone) before one did,
/// the server crashed, and the last line it printed is the reason.
async fn watch(stream: impl AsyncRead + Unpin, instances: Instances, id: String, stop: Arc<AtomicBool>, watch_exit: bool) {
    let mut lines = BufReader::new(stream).lines();
    let mut last = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if let Some(url) = detect_url(&line) {
            set(&instances, &id, Status::Running { url });
            return;
        }
        if !line.trim().is_empty() {
            last = line;
        }
    }
    let starting = instances.lock().unwrap_or_else(PoisonError::into_inner).get(&id).is_some_and(|i| i.status == Status::Starting);
    if watch_exit && starting && !stop.load(Ordering::SeqCst) {
        let error = if last.is_empty() { "It exited before printing a local address.".to_owned() } else { format!("It exited before printing a local address: {last}") };
        set(&instances, &id, Status::Crashed { error });
    }
}

impl DevServers {
    /// Starts `command` in `cwd` under `id`, replacing a server already running under it.
    pub fn start(&self, id: &str, cwd: &str, command: &str) -> Result<(), String> {
        self.stop(id);
        // `npm run dev` is not an executable of its own: it needs a shell.
        let mut cmd = if cfg!(windows) {
            let mut c = crate::proc::quiet("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = crate::proc::quiet("sh");
            c.args(["-c", command]);
            c
        };
        cmd.current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| {
            let error = format!("Failed to launch '{command}': {e}");
            self.instances.lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_owned(), Instance { status: Status::Crashed { error: error.clone() }, child: None, stop: Arc::default() });
            error
        })?;
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let stop = Arc::new(AtomicBool::new(false));
        self.instances.lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_owned(), Instance { status: Status::Starting, child: Some(child), stop: stop.clone() });
        // Most tools print their address on stdout; some use stderr, so both are read.
        if let Some(stdout) = stdout {
            tokio::spawn(watch(stdout, self.instances.clone(), id.to_owned(), stop.clone(), true));
        }
        if let Some(stderr) = stderr {
            tokio::spawn(watch(stderr, self.instances.clone(), id.to_owned(), stop, false));
        }
        Ok(())
    }

    pub fn stop(&self, id: &str) {
        if let Some(instance) = self.instances.lock().unwrap_or_else(PoisonError::into_inner).get_mut(id) {
            instance.stop.store(true, Ordering::SeqCst);
            if let Some(mut child) = instance.child.take() {
                let _ = child.start_kill();
            }
            instance.status = Status::Stopped;
        }
    }

    /// Stops every server. Called when the app quits: otherwise a dev server started from it keeps running as an
    /// orphan.
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.instances.lock().unwrap_or_else(PoisonError::into_inner).keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    #[must_use]
    pub fn status(&self, id: &str) -> Status {
        self.instances.lock().unwrap_or_else(PoisonError::into_inner).get(id).map_or(Status::Stopped, |i| i.status.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_address_is_found_in_what_each_tool_prints_and_loopback_is_normalized() {
        assert_eq!(detect_url("  ➜  Local:   http://localhost:5173/"), Some("http://localhost:5173".to_owned()));
        assert_eq!(detect_url("   - Local:        http://localhost:3000"), Some("http://localhost:3000".to_owned()));
        assert_eq!(detect_url("Server running at http://0.0.0.0:8080"), Some("http://127.0.0.1:8080".to_owned()));
        assert_eq!(detect_url("Compiled successfully!"), None);
    }

    #[test]
    fn the_command_follows_the_scripts_and_the_lockfile() {
        let dir = std::env::temp_dir().join(format!("viv-dev-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a temp folder");
        assert_eq!(detect_command(&dir), None, "not a Node project");
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"build":"x"}}"#).expect("writable");
        assert_eq!(detect_command(&dir), None, "neither script");
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"start":"x"}}"#).expect("writable");
        assert_eq!(detect_command(&dir).as_deref(), Some("npm start"));
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"start":"x","dev":"y"}}"#).expect("writable");
        assert_eq!(detect_command(&dir).as_deref(), Some("npm run dev"), "dev wins over start");
        std::fs::write(dir.join("pnpm-lock.yaml"), "").expect("writable");
        assert_eq!(detect_command(&dir).as_deref(), Some("pnpm dev"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_server_that_prints_its_address_is_running_and_one_that_exits_first_has_crashed() {
        let servers = DevServers::default();
        let (ok, bad) = if cfg!(windows) { ("echo Local: http://localhost:4321/ & ping -n 4 127.0.0.1 >nul", "echo boom") } else { ("echo Local: http://localhost:4321/; sleep 3", "echo boom") };
        let here = std::env::current_dir().expect("a current folder").display().to_string();
        servers.start("ok", &here, ok).expect("starts");
        servers.start("bad", &here, bad).expect("starts");
        for _ in 0..100 {
            if servers.status("ok") != Status::Starting && servers.status("bad") != Status::Starting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(servers.status("ok"), Status::Running { url: "http://localhost:4321".to_owned() });
        assert!(matches!(servers.status("bad"), Status::Crashed { error } if error.contains("boom")));
        servers.stop_all();
        assert_eq!(servers.status("ok"), Status::Stopped);
    }
}
