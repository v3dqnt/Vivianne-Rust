//! Finding, installing and checking the `agent-desktop` program. It is installed from the project's own GitHub
//! release into `~/.viv/bin`, only when the user asks, after its SHA-256 has been checked against the digest GitHub
//! publishes for the file. Nothing is installed system-wide, and nothing is run before that check passes.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const RELEASES: &str = "https://api.github.com/repos/vercel-labs/agent-desktop/releases/latest";
/// Where a download may come from: the project's own releases, nowhere else.
const DOWNLOAD_PREFIX: &str = "https://github.com/vercel-labs/agent-desktop/releases/download/";
/// The program is a few MB; anything much bigger is not it.
const MAX_BYTES: u64 = 40 * 1024 * 1024;

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "agent-desktop.exe"
    } else {
        "agent-desktop"
    }
}

fn bin_dir() -> PathBuf {
    crate::agent::system::paths::viv_home_path(&["bin"])
}

/// The program, from `~/.viv/bin` or else the PATH.
#[must_use]
pub fn installed_binary() -> Option<PathBuf> {
    let mine = bin_dir().join(binary_name());
    if mine.is_file() {
        return Some(mine);
    }
    which::which("agent-desktop").ok()
}

/// The release's name for this machine, in its file names.
fn target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return None,
    })
}

/// A release's file for this machine, with what it is checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub asset: String,
    pub url: String,
    pub size: u64,
    /// Lower-case hex SHA-256, from GitHub's digest for the file.
    pub sha256: String,
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
    browser_download_url: String,
}

/// Picks this machine's file out of a release's list.
fn pick(latest: Latest, target: &str) -> Result<Release, String> {
    let version = latest.tag_name.clone();
    let wanted = format!("agent-desktop-{version}-{target}.tar.gz");
    let asset = latest
        .assets
        .into_iter()
        .find(|a| a.name == wanted)
        .ok_or_else(|| format!("The release has no file for this machine ({wanted})."))?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_lowercase)
        .ok_or("The release gives no checksum for that file, so it is not installed.")?;
    if !asset.browser_download_url.starts_with(DOWNLOAD_PREFIX) {
        return Err(
            "The release points outside the project's own downloads; not installing.".to_owned(),
        );
    }
    Ok(Release {
        version,
        asset: asset.name,
        url: asset.browser_download_url,
        size: asset.size,
        sha256,
    })
}

/// The newest release's file for this machine.
///
/// # Errors
/// No network, no file for this machine, or no checksum for it.
pub async fn find_release() -> Result<Release, String> {
    let target = target().ok_or("There is no agent-desktop build for this operating system.")?;
    let client = reqwest::Client::builder()
        .user_agent("vivianne")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let latest: Latest = client
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GitHub said no: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Couldn't read the release: {e}"))?;
    pick(latest, target)
}

/// Downloads a release, checks it, and installs the program into `~/.viv/bin`. Returns where it went.
///
/// # Errors
/// The download failed, the checksum does not match (nothing is kept then), or the archive could not be opened.
pub async fn download(release: &Release) -> Result<PathBuf, String> {
    if !release.url.starts_with(DOWNLOAD_PREFIX) || release.size > MAX_BYTES {
        return Err(
            "Not installing: that file is not from the project's releases or is too big."
                .to_owned(),
        );
    }
    let bytes = reqwest::Client::builder()
        .user_agent("vivianne")
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?
        .get(&release.url)
        .send()
        .await
        .map_err(|e| format!("The download failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("The download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("The download failed: {e}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return Err("The download is bigger than expected; not installing.".to_owned());
    }
    verify(&bytes, &release.sha256)?;
    let scratch = std::env::temp_dir().join(format!("viv-agent-desktop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let result = install_from(&bytes, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// The checksum of what arrived against the one it was published with.
pub(crate) fn verify(bytes: &[u8], expected: &str) -> Result<(), String> {
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "The download does not match its published checksum (got {}…); nothing was installed.",
            &got[..12]
        ))
    }
}

/// Unpacks the archive in `scratch` and moves the program into `~/.viv/bin`. The system's `tar` does the unpacking
/// (Windows, macOS and Linux all ship one).
fn install_from(bytes: &[u8], scratch: &Path) -> Result<PathBuf, String> {
    let archive = scratch.join("agent-desktop.tar.gz");
    std::fs::write(&archive, bytes).map_err(|e| e.to_string())?;
    let out = crate::proc::quiet_std("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(scratch)
        .output()
        .map_err(|e| format!("Couldn't run tar to open the download: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't open the download: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let unpacked = scratch.join(binary_name());
    if !unpacked.is_file() {
        return Err("The download did not contain the program.".to_owned());
    }
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(binary_name());
    // A running copy cannot be replaced on Windows; the old one is moved aside first.
    if target.exists() {
        let _ = std::fs::rename(&target, dir.join(format!("{}.old", binary_name())));
    }
    std::fs::copy(&unpacked, &target).map_err(|e| format!("Couldn't install the program: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    Ok(target)
}

/// What the program says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub version: String,
    pub platform: String,
    /// `granted`, `denied`, `not_required`, or whatever the program reports.
    pub accessibility: String,
}

/// Asks the program for its status.
///
/// # Errors
/// It would not start or did not answer in the expected form.
pub async fn status(binary: &Path) -> Result<Status, String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        crate::proc::quiet(binary)
            .arg("status")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "agent-desktop did not answer.".to_owned())?
    .map_err(|e| format!("Couldn't run agent-desktop: {e}"))?;
    parse_status(&String::from_utf8_lossy(&out.stdout))
}

fn parse_status(text: &str) -> Result<Status, String> {
    let json: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|_| "agent-desktop did not answer with a status.".to_owned())?;
    if json["ok"] != serde_json::Value::Bool(true) {
        return Err(json["error"]["message"]
            .as_str()
            .unwrap_or("agent-desktop reported a problem.")
            .to_owned());
    }
    let data = &json["data"];
    Ok(Status {
        version: data["version"].as_str().unwrap_or("?").to_owned(),
        platform: data["platform"].as_str().unwrap_or("?").to_owned(),
        accessibility: data["permissions"]["accessibility"]["state"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest(digest: Option<&str>, url: &str) -> Latest {
        Latest {
            tag_name: "v1.3.1".into(),
            assets: vec![
                Asset {
                    name: "agent-desktop-v1.3.1-x86_64-pc-windows-msvc.tar.gz".into(),
                    size: 1_835_706,
                    digest: digest.map(str::to_owned),
                    browser_download_url: url.into(),
                },
                Asset {
                    name: "agent-desktop-ffi-v1.3.1-x86_64-pc-windows-msvc.zip".into(),
                    size: 1,
                    digest: None,
                    browser_download_url: "x".into(),
                },
            ],
        }
    }

    #[test]
    fn this_machines_file_is_picked_with_its_checksum_and_only_from_the_projects_downloads() {
        let url =
            format!("{DOWNLOAD_PREFIX}v1.3.1/agent-desktop-v1.3.1-x86_64-pc-windows-msvc.tar.gz");
        let release = pick(
            latest(Some("sha256:ABC123"), &url),
            "x86_64-pc-windows-msvc",
        )
        .expect("picks");
        assert_eq!(
            (release.sha256.as_str(), release.size),
            ("abc123", 1_835_706)
        );
        assert!(
            pick(latest(None, &url), "x86_64-pc-windows-msvc").is_err(),
            "no checksum: not installed"
        );
        assert!(
            pick(
                latest(Some("sha256:ab"), "https://evil.example/x.tar.gz"),
                "x86_64-pc-windows-msvc"
            )
            .is_err()
        );
        assert!(
            pick(latest(Some("sha256:ab"), &url), "aarch64-apple-darwin").is_err(),
            "no file for another machine"
        );
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_refused() {
        let good: String = Sha256::digest(b"program")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(verify(b"program", &good).is_ok());
        assert!(verify(b"tampered", &good).is_err());
    }

    #[test]
    fn the_programs_status_is_read_and_a_failure_is_reported() {
        let ok = r#"{"ok":true,"data":{"version":"1.3.1","platform":"windows","permissions":{"accessibility":{"state":"granted"}}}}"#;
        assert_eq!(
            parse_status(ok),
            Ok(Status {
                version: "1.3.1".into(),
                platform: "windows".into(),
                accessibility: "granted".into()
            })
        );
        assert!(
            parse_status(r#"{"ok":false,"error":{"message":"no"}}"#).is_err()
                && parse_status("garbage").is_err()
        );
    }
}
