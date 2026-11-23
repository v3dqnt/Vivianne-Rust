//! Running the system `ssh`, as herdr does: OpenSSH keeps authentication, host keys, jump hosts and the agent, and
//! this module only chooses its options. No Rust SSH library is used.
//!
//! The options are herdr's: compression, a managed config that first includes the user's own, keepalives that the
//! user's settings override, a private control socket for connection reuse (not on Windows, whose OpenSSH has none),
//! and for background connections no prompts at all and strict host-key checking.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A noninteractive command that has not finished by now is given up on.
pub const NONINTERACTIVE_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
/// Most of a noninteractive command's stderr that is kept.
pub const NONINTERACTIVE_STDERR_LIMIT: usize = 16 * 1024;
/// A shared connection stays open this many seconds after its last user.
pub const CONTROL_PERSIST_SECONDS: u32 = 600;
pub const SERVER_ALIVE_INTERVAL: u32 = 15;
pub const SERVER_ALIVE_COUNT_MAX: u32 = 4;
/// What the remote bridge prints when it is ready; anything before it (a login banner) is dropped.
pub const REMOTE_OUTPUT_READY_MARKER: &str = "viv-remote-output-ready:1";
/// The target that runs the bridge on this computer instead of over SSH. For testing and for two sessions on one
/// machine; it is not a way around SSH to a real host.
pub const LOOPBACK_TARGET: &str = "viv-loopback";

/// How `ssh` is started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshOptions {
    /// Background connections: no prompts, strict host keys, short timeouts.
    pub noninteractive: bool,
    /// A config that includes the user's, then adds keepalives. `None` runs plain ssh.
    pub managed_config: Option<PathBuf>,
    /// The private control socket for connection reuse (not on Windows).
    pub control_path: Option<PathBuf>,
}

/// The program to run: `ssh` from the PATH, or `VIV_SSH` when set.
#[must_use]
pub fn program() -> String {
    std::env::var("VIV_SSH").unwrap_or_else(|_| "ssh".to_owned())
}

/// The arguments for `ssh` to run `remote_command` (or a login shell when `None`) on `target`.
#[must_use]
pub fn args(opts: &SshOptions, target: &str, remote_command: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec!["-C".into()];
    if let Some(cfg) = &opts.managed_config {
        a.push("-F".into());
        a.push(cfg.display().to_string());
    }
    if let Some(ctl) = &opts.control_path {
        a.push("-S".into());
        a.push(ctl.display().to_string());
        a.push("-o".into());
        a.push("ControlMaster=auto".into());
        a.push("-o".into());
        a.push(format!("ControlPersist={CONTROL_PERSIST_SECONDS}"));
    }
    if opts.noninteractive {
        for o in [
            "BatchMode=yes",
            "NumberOfPasswordPrompts=0",
            "StrictHostKeyChecking=yes",
            "ConnectTimeout=10",
            "ConnectionAttempts=1",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=4",
        ] {
            a.push("-o".into());
            a.push(o.into());
        }
    }
    a.push("-T".into());
    a.push(target.to_owned());
    if let Some(cmd) = remote_command {
        a.push(cmd.to_owned());
    }
    a
}

/// What the managed config says: the user's own config first (so their keepalives win), then ours.
#[must_use]
pub fn managed_config_text(user_config: Option<&Path>) -> String {
    let mut text = String::new();
    if let Some(p) = user_config {
        // Quoted, so a path with spaces (a Windows profile) stays one argument.
        text.push_str(&format!(
            "Include \"{}\"\n",
            p.display().to_string().replace('\\', "/")
        ));
    }
    text.push_str(&format!(
        "Host *\n  ServerAliveInterval {SERVER_ALIVE_INTERVAL}\n  ServerAliveCountMax {SERVER_ALIVE_COUNT_MAX}\n"
    ));
    text
}

/// Writes the managed config under `~/.viv/client/ssh/` and returns its path.
pub fn write_managed_config() -> Result<PathBuf, String> {
    let dir = vivianne_ai::paths::viv_home_path(&["client", "ssh"]);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let user = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join(".ssh").join("config"))
        .filter(|p| p.is_file());
    let path = dir.join("config");
    std::fs::write(&path, managed_config_text(user.as_deref())).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The options for a connection from this machine. `managed` is the `[remote].manage_ssh_config` switch.
pub fn options(noninteractive: bool, managed: bool) -> SshOptions {
    if !managed {
        return SshOptions {
            noninteractive,
            ..SshOptions::default()
        };
    }
    SshOptions {
        noninteractive,
        managed_config: write_managed_config().ok(),
        control_path: (!cfg!(windows))
            .then(|| vivianne_ai::paths::viv_home_path(&["client", "ssh", "ctl"])),
    }
}

/// A word for a remote shell: left bare when it is only safe characters, otherwise single-quoted.
#[must_use]
pub fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%_+=:,./-".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The command that starts the remote bridge on a POSIX host. It prints the ready marker first so that a login
/// banner cannot be mistaken for protocol.
#[must_use]
pub fn remote_bridge_command(session: &str, idle_timeout: bool) -> String {
    let idle = if idle_timeout {
        " --idle-timeout-v1"
    } else {
        ""
    };
    format!(
        "printf '\\n%s\\n' '{REMOTE_OUTPUT_READY_MARKER}'\nexec \"$HOME/.local/bin/viv\" --session {} remote-client-bridge{idle}",
        shell_quote(session)
    )
}

/// Standard base64 of `bytes`.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The command that starts the bridge on a Windows host: PowerShell with an encoded script (UTF-16LE, base64), so no
/// quoting of the script can go wrong. It prints the ready marker, runs the bridge and exits with its code.
#[must_use]
pub fn remote_bridge_command_windows(session: &str, idle_timeout: bool) -> String {
    let script = format!(
        "[Console]::Out.WriteLine(''); [Console]::Out.WriteLine('{REMOTE_OUTPUT_READY_MARKER}'); $p = Start-Process -FilePath \"$env:USERPROFILE/.local/bin/viv.exe\" -ArgumentList '--session','{session}','remote-client-bridge'{} -NoNewWindow -PassThru; $p.WaitForExit(); exit $p.ExitCode",
        if idle_timeout {
            ",'--idle-timeout-v1'"
        } else {
            ""
        }
    );
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
        base64(&utf16)
    )
}

/// Drops everything up to and including the line with the ready marker. `None` while it has not arrived.
#[must_use]
pub fn after_ready_marker(output: &[u8]) -> Option<&[u8]> {
    let marker = REMOTE_OUTPUT_READY_MARKER.as_bytes();
    let at = output.windows(marker.len()).position(|w| w == marker)?;
    let rest = &output[at + marker.len()..];
    // The marker's own line ends with a newline.
    Some(
        rest.strip_prefix(b"\r\n")
            .or_else(|| rest.strip_prefix(b"\n"))
            .unwrap_or(rest),
    )
}

/// Why a connection failed, as far as its message says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The host key is unknown or has changed: the person must look at it.
    HostKey,
    /// The key or password was not accepted: the person must sign in once in a terminal.
    Auth,
    /// The remote is a platform there is no build for.
    Platform,
    /// The remote server speaks another protocol, or the handshake failed.
    Protocol,
    /// Anything else: the network, most likely. Retried with backoff.
    Other,
}

impl Failure {
    /// Whether the person has to do something. Everything else retries by itself.
    #[must_use]
    pub fn needs_attention(self) -> bool {
        self != Self::Other
    }
}

/// Sorts an error message from `ssh` or from the bridge.
#[must_use]
pub fn classify(message: &str) -> Failure {
    let m = message.to_ascii_lowercase();
    if m.contains("host key verification failed")
        || m.contains("remote host identification has changed")
        || m.contains("no matching host key")
    {
        Failure::HostKey
    } else if m.contains("permission denied")
        && ["(publickey", "(keyboard-interactive", "(password"]
            .iter()
            .any(|k| m.contains(k))
    {
        Failure::Auth
    } else if m.contains("unsupported remote platform") {
        Failure::Platform
    } else if ["protocol", "handshake", "install or update", "not ready"]
        .iter()
        .any(|k| m.contains(k))
    {
        Failure::Protocol
    } else {
        Failure::Other
    }
}

/// The remote's operating system and CPU, as the build names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlatform {
    pub os: &'static str,
    pub arch: &'static str,
}

impl RemotePlatform {
    /// From `uname -s` and `uname -m`.
    pub fn from_uname(os: &str, arch: &str) -> Result<Self, String> {
        let os = match os.trim() {
            "Linux" => "linux",
            "Darwin" => "macos",
            other => {
                return Err(format!(
                    "unsupported remote platform: {other} {}",
                    arch.trim()
                ));
            }
        };
        let arch = match arch.trim() {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            other => return Err(format!("unsupported remote platform: {os} {other}")),
        };
        Ok(Self { os, arch })
    }

    /// From the Windows probe's one line, `viv-windows:<arch>`. ARM64 runs the x64 build, best effort.
    pub fn from_windows_probe(line: &str) -> Result<Self, String> {
        let arch = line.trim().strip_prefix("viv-windows:").unwrap_or_default();
        match arch.to_ascii_uppercase().as_str() {
            "AMD64" | "X86_64" | "ARM64" => Ok(Self {
                os: "windows",
                arch: "x86_64",
            }),
            other => Err(format!("unsupported remote platform: Windows {other}")),
        }
    }

    /// The release file this platform needs.
    #[must_use]
    pub fn release_file(&self) -> String {
        let ext = if self.os == "windows" { ".exe" } else { "" };
        format!("viv-{}-{}{ext}", self.os, self.arch)
    }
}

/// Where a POSIX host is searched for an installed `viv`, after the PATH.
pub const KNOWN_REMOTE_PATHS: [&str; 7] = [
    "$HOME/.local/bin/viv",
    "/opt/homebrew/bin/viv",
    "/usr/local/bin/viv",
    "/home/linuxbrew/.linuxbrew/bin/viv",
    "$HOME/.nix-profile/bin/viv",
    "/nix/var/nix/profiles/default/bin/viv",
    "/run/current-system/sw/bin/viv",
];

/// A script (read by `/bin/sh -s`) that prints each candidate `viv` it finds, one per line.
#[must_use]
pub fn find_candidates_script() -> String {
    let mut s = String::from("command -v viv 2>/dev/null\n");
    for p in KNOWN_REMOTE_PATHS {
        s.push_str(&format!("[ -x \"{p}\" ] && printf '%s\\n' \"{p}\"\n"));
    }
    s.push_str("true\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interactive_command_is_herdrs_exact_argv() {
        let opts = SshOptions {
            noninteractive: false,
            managed_config: Some("cfg".into()),
            control_path: Some("ctl".into()),
        };
        assert_eq!(
            args(&opts, "workbox", None),
            [
                "-C",
                "-F",
                "cfg",
                "-S",
                "ctl",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=600",
                "-T",
                "workbox"
            ]
        );
        let plain = args(&SshOptions::default(), "workbox", Some("echo hi"));
        assert_eq!(plain, ["-C", "-T", "workbox", "echo hi"]);
    }

    #[test]
    fn a_background_command_never_prompts_and_checks_host_keys_strictly() {
        let opts = SshOptions {
            noninteractive: true,
            ..SshOptions::default()
        };
        let a = args(&opts, "workbox", Some("x")).join(" ");
        for needed in [
            "BatchMode=yes",
            "NumberOfPasswordPrompts=0",
            "StrictHostKeyChecking=yes",
            "ConnectTimeout=10",
            "ConnectionAttempts=1",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=4",
        ] {
            assert!(a.contains(needed), "{needed}");
        }
        assert!(
            !a.contains("ForwardAgent"),
            "agent forwarding is never switched on by us"
        );
    }

    #[test]
    fn the_managed_config_includes_the_users_first_and_sets_no_control_options() {
        let text = managed_config_text(Some(Path::new("C:\\Users\\me\\.ssh\\config")));
        assert!(text.starts_with("Include \"C:/Users/me/.ssh/config\"\n"));
        assert!(text.contains("ServerAliveInterval 15") && text.contains("ServerAliveCountMax 4"));
        assert!(
            !text.contains("Control"),
            "control options go on the command line"
        );
    }

    #[test]
    fn the_remote_command_quotes_the_session_and_the_ready_marker_skips_a_banner() {
        assert_eq!(shell_quote("agents-1"), "agents-1");
        assert_eq!(shell_quote("a b'c"), r"'a b'\''c'");
        let cmd = remote_bridge_command("default", true);
        assert!(
            cmd.contains("viv-remote-output-ready:1")
                && cmd.ends_with("remote-client-bridge --idle-timeout-v1")
        );
        let out = b"Welcome to box\r\nviv-remote-output-ready:1\n{\"frame\"";
        assert_eq!(after_ready_marker(out), Some(&b"{\"frame\""[..]));
        assert_eq!(after_ready_marker(b"banner only"), None);
    }

    #[test]
    fn a_windows_host_gets_an_encoded_powershell_command_that_carries_only_a_checked_session_name()
    {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        let cmd = remote_bridge_command_windows("default", true);
        assert!(
            cmd.starts_with("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ")
        );
        assert!(
            !cmd.contains("remote-client-bridge"),
            "the script is encoded, not visible to a quoting layer"
        );
    }

    #[test]
    fn failures_are_sorted_by_what_the_person_must_do() {
        assert_eq!(classify("Host key verification failed."), Failure::HostKey);
        assert_eq!(
            classify("@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @"),
            Failure::HostKey
        );
        assert_eq!(
            classify("me@box: Permission denied (publickey,password)."),
            Failure::Auth
        );
        assert_eq!(
            classify("unsupported remote platform: Linux riscv64"),
            Failure::Platform
        );
        assert_eq!(classify("handshake failed"), Failure::Protocol);
        assert_eq!(classify("Connection timed out"), Failure::Other);
        assert!(Failure::Auth.needs_attention() && !Failure::Other.needs_attention());
    }

    #[test]
    fn remote_platforms_map_from_uname_and_the_windows_probe() {
        let p = RemotePlatform::from_uname("Linux\n", "aarch64\n").expect("linux arm");
        assert_eq!(p.release_file(), "viv-linux-aarch64");
        assert_eq!(
            RemotePlatform::from_uname("Darwin", "arm64")
                .expect("mac")
                .arch,
            "aarch64"
        );
        assert!(RemotePlatform::from_uname("Linux", "riscv64").is_err());
        assert!(RemotePlatform::from_uname("FreeBSD", "x86_64").is_err());
        let w = RemotePlatform::from_windows_probe("viv-windows:ARM64\n").expect("arm64 runs x64");
        assert_eq!(w.release_file(), "viv-windows-x86_64.exe");
        assert!(RemotePlatform::from_windows_probe("viv-windows:IA64").is_err());
        assert!(find_candidates_script().contains("command -v viv"));
    }
}
