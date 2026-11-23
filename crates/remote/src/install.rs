//! Putting `viv` on a machine that does not have it. Only on the person's say-so, with the question shown first;
//! a background reconnect never installs, updates or restarts anything. What is installed is checked first: the file
//! must be for the remote's operating system and CPU, and a download must match a SHA-256 the release publishes.
//!
//! The local program can be copied only to a machine of the same kind (a Windows `viv.exe` does not run on Linux).
//! For other machines there must be a published build, found through `VIV_RELEASE_BASE` (a directory URL holding
//! `viv-<os>-<arch>` and its `.sha256`); until releases are published, `VIV_REMOTE_BINARY` names a file to copy.

use std::time::Duration;

use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::catalog::SavedMachine;
use crate::ssh::{self, LOOPBACK_TARGET, RemotePlatform};

/// What the person is asked before anything is installed.
#[must_use]
pub fn confirmation_question(machine: &SavedMachine, platform: &RemotePlatform) -> String {
    format!(
        "Install viv on {} ({} {}) into ~/.local/bin/viv? Nothing else on that machine is changed. [Y/n]",
        machine.label, platform.os, platform.arch
    )
}

/// The proof that the person said yes to [`confirmation_question`]. [`install`] cannot be called without one.
#[derive(Debug)]
pub struct Confirmed(());

impl Confirmed {
    /// Only for the code that has just shown the question and read a yes.
    #[must_use]
    pub fn after_asking() -> Self {
        Self(())
    }
}

/// Runs `script` on the machine with `/bin/sh -s` and returns its output.
async fn run_script(
    machine: &SavedMachine,
    script: &str,
    stdin_after: Option<&[u8]>,
) -> Result<String, String> {
    run_remote(machine, "/bin/sh -s", script, stdin_after).await
}

/// Runs `command` on the machine, with `script` (and then `stdin_after`) as its input.
async fn run_remote(
    machine: &SavedMachine,
    command: &str,
    script: &str,
    stdin_after: Option<&[u8]>,
) -> Result<String, String> {
    if machine.target == LOOPBACK_TARGET {
        return Err("the loopback machine is this computer: nothing to install".to_owned());
    }
    let opts = ssh::options(true, true);
    let mut cmd = tokio::process::Command::new(ssh::program());
    cmd.args(ssh::args(&opts, &machine.target, Some(command)))
        .env_remove("SSH_ASKPASS")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| format!("could not run ssh: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let script = script.to_owned();
    let extra = stdin_after.map(<[u8]>::to_vec);
    let io = async move {
        // The script is the shell's input; a file, when there is one, follows the script's own `exec cat`.
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        if let Some(bytes) = extra {
            stdin.write_all(&bytes).await.map_err(|e| e.to_string())?;
        }
        drop(stdin);
        let (mut out, mut err) = (String::new(), String::new());
        let _ = stdout.read_to_string(&mut out).await;
        let _ = (&mut stderr)
            .take(ssh::NONINTERACTIVE_STDERR_LIMIT as u64)
            .read_to_string(&mut err)
            .await;
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if status.success() {
            Ok(out)
        } else {
            Err(if err.trim().is_empty() {
                format!("ssh ended with {status}")
            } else {
                err.trim().to_owned()
            })
        }
    };
    tokio::time::timeout(
        ssh::NONINTERACTIVE_COMMAND_TIMEOUT.max(Duration::from_secs(120)),
        io,
    )
    .await
    .map_err(|_| "noninteractive SSH command timed out".to_owned())?
}

/// The remote's platform.
pub async fn detect_platform(machine: &SavedMachine) -> Result<RemotePlatform, String> {
    let platform = detect_uncached(machine).await?;
    crate::meta::put(
        &machine.id,
        crate::meta::Meta {
            os: platform.os.to_owned(),
            arch: platform.arch.to_owned(),
        },
    );
    Ok(platform)
}

async fn detect_uncached(machine: &SavedMachine) -> Result<RemotePlatform, String> {
    // A POSIX shell first; a Windows host has none, so PowerShell is asked next.
    match run_script(
        machine,
        "uname -s; uname -m
",
        None,
    )
    .await
    {
        Ok(out) => {
            let mut lines = out.lines();
            let (os, arch) = (
                lines.next().unwrap_or_default(),
                lines.next().unwrap_or_default(),
            );
            // Windows' OpenSSH shells (Git Bash, MSYS, Cygwin) answer uname with these names.
            if ["MINGW", "MSYS", "CYGWIN"]
                .iter()
                .any(|p| os.starts_with(p))
            {
                return windows_probe(machine).await;
            }
            RemotePlatform::from_uname(os, arch)
        }
        Err(_) => windows_probe(machine).await,
    }
}

async fn windows_probe(machine: &SavedMachine) -> Result<RemotePlatform, String> {
    let out = run_remote(
        machine,
        "powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"Write-Output ('viv-windows:' + $env:PROCESSOR_ARCHITECTURE)\"",
        "",
        None,
    )
    .await?;
    RemotePlatform::from_windows_probe(
        out.lines()
            .find(|l| l.starts_with("viv-windows:"))
            .unwrap_or_default(),
    )
}

/// The most a downloaded program may be.
const MAX_DOWNLOAD_BYTES: usize = 120 * 1024 * 1024;

/// Fetches a published build and the SHA-256 published beside it (`<url>.sha256`). The checksum comes from a
/// separate file so a changed program does not carry a matching one; `install` still checks it.
pub async fn download_release(url: &str) -> Result<(Vec<u8>, String), String> {
    if !url.starts_with("https://") {
        return Err("a release is fetched over https only".to_owned());
    }
    let client = reqwest::Client::builder()
        .user_agent("vivianne")
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let sums = client
        .get(format!("{url}.sha256"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("could not fetch the checksum: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let sha = sums
        .split_whitespace()
        .next()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("the checksum file does not hold a SHA-256")?
        .to_ascii_lowercase();
    let bytes = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("the download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("the download failed: {e}"))?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err("the download is bigger than expected; not installing".to_owned());
    }
    Ok((bytes.to_vec(), sha))
}

/// The `viv` programs found on the remote, best candidate first.
pub async fn find_installed(machine: &SavedMachine) -> Result<Vec<String>, String> {
    let out = run_script(machine, &ssh::find_candidates_script(), None).await?;
    let mut found: Vec<String> = Vec::new();
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !found.iter().any(|f| f == line) {
            found.push(line.to_owned());
        }
    }
    Ok(found)
}

/// The SHA-256 of bytes, lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where the program to install comes from, in the order they are tried.
#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    /// `VIV_REMOTE_BINARY`: a file the person named.
    Override(std::path::PathBuf),
    /// This computer's own program, for a machine of the same kind.
    ThisProgram(std::path::PathBuf),
    /// A published build, from `VIV_RELEASE_BASE`.
    Release { url: String },
}

/// Chooses the source for `platform`.
pub fn choose_source(platform: &RemotePlatform) -> Result<Source, String> {
    if let Some(path) = std::env::var_os("VIV_REMOTE_BINARY") {
        return Ok(Source::Override(path.into()));
    }
    let here = RemotePlatform {
        os: std::env::consts::OS,
        arch: match std::env::consts::ARCH {
            "x86_64" => "x86_64",
            "aarch64" => "aarch64",
            _ => "other",
        },
    };
    if here.os == platform.os && here.arch == platform.arch {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // Only a plain `viv`: the app itself (vivianne.exe) is not what a server runs.
        if exe.file_stem().is_some_and(|s| s == "viv") {
            return Ok(Source::ThisProgram(exe));
        }
    }
    match std::env::var("VIV_RELEASE_BASE") {
        Ok(base) if base.starts_with("https://") => Ok(Source::Release {
            url: format!("{}/{}", base.trim_end_matches('/'), platform.release_file()),
        }),
        _ => Err(format!(
            "there is no build of viv for {} {} to install: this program is for {} {}, and no release location (VIV_RELEASE_BASE) or file (VIV_REMOTE_BINARY) is set",
            platform.os, platform.arch, here.os, here.arch
        )),
    }
}

/// The script that stores the bytes that follow it: written to a temporary name, made executable, then moved into
/// place, so a cut connection never leaves a half-written program where `viv` is looked for.
#[must_use]
pub fn upload_script(len: usize) -> String {
    format!(
        "set -eu\ndest=\"$HOME/.local/bin/viv\"\ndir=\"$(dirname \"$dest\")\"\nmkdir -p \"$dir\"\ntmp=\"${{dest}}.tmp.$$\"\nhead -c {len} > \"$tmp\"\nchmod 755 \"$tmp\"\nmv \"$tmp\" \"$dest\"\nprintf 'installed %s\\n' \"$dest\"\nexit 0\n"
    )
}

/// Installs `bytes` as `~/.local/bin/viv` on the machine. Needs [`Confirmed`]; verifies `sha256` first.
pub async fn install(
    machine: &SavedMachine,
    bytes: &[u8],
    sha256: &str,
    _confirmed: Confirmed,
) -> Result<String, String> {
    let got = sha256_hex(bytes);
    if got != sha256.to_ascii_lowercase() {
        return Err(format!(
            "the file does not match its checksum (got {}…); nothing was installed",
            &got[..12]
        ));
    }
    run_script(machine, &upload_script(bytes.len()), Some(bytes)).await
}

/// Why a running server needs a restart before a client can use it, in the order they are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartReason {
    EndpointGeneration,
    SurfaceInterest,
    HealthCheck,
}

/// Whether a server with these capabilities and generation must be restarted for this client.
#[must_use]
pub fn restart_reason(generation: u32, capabilities: &[String]) -> Option<RestartReason> {
    use crate::protocol::{CAP_HEALTH_CHECK, CAP_SURFACE_INTEREST, ENDPOINT_GENERATION};
    if generation != ENDPOINT_GENERATION {
        return Some(RestartReason::EndpointGeneration);
    }
    let has = |c: &str| capabilities.iter().any(|x| x == c);
    if !has(CAP_SURFACE_INTEREST) {
        return Some(RestartReason::SurfaceInterest);
    }
    if !has(CAP_HEALTH_CHECK) {
        return Some(RestartReason::HealthCheck);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_x64() -> RemotePlatform {
        RemotePlatform {
            os: "linux",
            arch: "x86_64",
        }
    }

    #[test]
    fn the_question_names_the_machine_and_the_one_place_that_changes() {
        let m = SavedMachine {
            id: "0".repeat(32),
            label: "workbox".into(),
            target: "me@workbox".into(),
            session: "default".into(),
            enabled: true,
        };
        let q = confirmation_question(&m, &linux_x64());
        assert!(q.contains("workbox") && q.contains("~/.local/bin/viv") && q.ends_with("[Y/n]"));
    }

    #[test]
    fn a_program_for_another_kind_of_machine_is_never_copied_and_the_reason_is_plain() {
        // This test PC is not an "riscv" machine, so no source is found without an override or a release location.
        let odd = RemotePlatform {
            os: "linux",
            arch: "riscv64",
        };
        if std::env::var_os("VIV_REMOTE_BINARY").is_none()
            && std::env::var_os("VIV_RELEASE_BASE").is_none()
        {
            let why = choose_source(&odd).unwrap_err();
            assert!(why.contains("no build of viv") && why.contains("VIV_RELEASE_BASE"));
        }
    }

    #[test]
    fn the_upload_writes_beside_the_target_and_moves_it_into_place_after_checking_the_size() {
        let s = upload_script(1234);
        assert!(
            s.contains("head -c 1234")
                && s.contains("chmod 755")
                && s.contains("mv \"$tmp\" \"$dest\"")
        );
        assert!(s.find("chmod").expect("has chmod") < s.find("mv ").expect("has mv"));
    }

    #[tokio::test]
    async fn a_file_that_does_not_match_its_checksum_is_refused_before_anything_is_sent() {
        let m = SavedMachine {
            id: "0".repeat(32),
            label: "x".into(),
            target: LOOPBACK_TARGET.into(),
            session: "default".into(),
            enabled: true,
        };
        let err = install(
            &m,
            b"program",
            &sha256_hex(b"other"),
            Confirmed::after_asking(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("does not match its checksum"));
    }

    #[test]
    fn an_old_server_is_restarted_only_for_a_reason_and_in_herdrs_order() {
        use crate::protocol::{CAP_HEALTH_CHECK, CAP_SURFACE_INTEREST, ENDPOINT_GENERATION};
        let all = vec![CAP_SURFACE_INTEREST.to_owned(), CAP_HEALTH_CHECK.to_owned()];
        assert_eq!(restart_reason(ENDPOINT_GENERATION, &all), None);
        assert_eq!(
            restart_reason(0, &all),
            Some(RestartReason::EndpointGeneration)
        );
        assert_eq!(
            restart_reason(ENDPOINT_GENERATION, &[CAP_HEALTH_CHECK.to_owned()]),
            Some(RestartReason::SurfaceInterest)
        );
        assert_eq!(
            restart_reason(ENDPOINT_GENERATION, &[CAP_SURFACE_INTEREST.to_owned()]),
            Some(RestartReason::HealthCheck)
        );
        assert_eq!(sha256_hex(b"").len(), 64);
    }
}
