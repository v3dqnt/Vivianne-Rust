//! Finding, installing and checking the `agent-browser` program. It is one file per platform on the project's GitHub
//! release; it is installed into `~/.viv/bin` only when the user asks, after its SHA-256 has been checked against the
//! digest GitHub publishes for the file.

use std::path::PathBuf;

use serde::Deserialize;

use crate::computer::Release;
use crate::computer::install::verify;

const RELEASES: &str = "https://api.github.com/repos/vercel-labs/agent-browser/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/vercel-labs/agent-browser/releases/download/";
const MAX_BYTES: u64 = 60 * 1024 * 1024;

fn file_name() -> &'static str {
    if cfg!(windows) {
        "agent-browser.exe"
    } else {
        "agent-browser"
    }
}

fn bin_dir() -> PathBuf {
    crate::host::viv_home_path(&["bin"])
}

/// The program, from `~/.viv/bin` or else the PATH.
#[must_use]
pub fn installed_binary() -> Option<PathBuf> {
    let mine = bin_dir().join(file_name());
    if mine.is_file() {
        return Some(mine);
    }
    which::which("agent-browser").ok()
}

/// The release's name for this machine's file.
fn asset_name() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "agent-browser-win32-x64.exe",
        ("macos", "x86_64") => "agent-browser-darwin-x64",
        ("macos", "aarch64") => "agent-browser-darwin-arm64",
        ("linux", "x86_64") => "agent-browser-linux-x64",
        ("linux", "aarch64") => "agent-browser-linux-arm64",
        _ => return None,
    })
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

fn pick(latest: Latest, wanted: &str) -> Result<Release, String> {
    let version = latest.tag_name;
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
    let wanted =
        asset_name().ok_or("There is no agent-browser build for this operating system.")?;
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
    pick(latest, wanted)
}

/// Downloads a release, checks it, and installs the program into `~/.viv/bin`. Returns where it went.
///
/// # Errors
/// The download failed or does not match its published checksum (nothing is kept then).
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
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("The download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("The download failed: {e}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return Err("The download is bigger than expected; not installing.".to_owned());
    }
    verify(&bytes, &release.sha256)?;
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(file_name());
    // A running copy cannot be replaced on Windows; the old one is moved aside first.
    if target.exists() {
        let _ = std::fs::rename(&target, dir.join(format!("{}.old", file_name())));
    }
    std::fs::write(&target, &bytes).map_err(|e| format!("Couldn't install the program: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    Ok(target)
}

/// What the program says about itself: its version, and which browser it will drive.
///
/// # Errors
/// It would not start or did not answer.
pub async fn status(binary: &std::path::Path) -> Result<String, String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        crate::proc::quiet(binary)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "agent-browser did not answer.".to_owned())?
    .map_err(|e| format!("Couldn't run agent-browser: {e}"))?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if version.is_empty() {
        return Err("agent-browser did not say which version it is.".to_owned());
    }
    let browser = super::system_browser().map_or_else(
        || "no Chrome or Edge found (it would need to download its own)".to_owned(),
        |p| format!("will use {}", p.display()),
    );
    Ok(format!("{version}; {browser}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest(digest: Option<&str>, url: &str) -> Latest {
        Latest {
            tag_name: "v0.43.0".into(),
            assets: vec![Asset {
                name: "agent-browser-win32-x64.exe".into(),
                size: 20_043_776,
                digest: digest.map(str::to_owned),
                browser_download_url: url.into(),
            }],
        }
    }

    #[test]
    fn this_machines_file_is_picked_with_its_checksum_and_only_from_the_projects_downloads() {
        let win = "agent-browser-win32-x64.exe";
        let url = format!("{DOWNLOAD_PREFIX}v0.43.0/{win}");
        let release = pick(latest(Some("sha256:ABC123"), &url), win).expect("picks");
        assert_eq!(
            (release.sha256.as_str(), release.size),
            ("abc123", 20_043_776)
        );
        assert!(pick(latest(None, &url), win).is_err());
        assert!(pick(latest(Some("sha256:ab"), "https://evil.example/x.exe"), win).is_err());
        assert!(pick(latest(Some("sha256:ab"), &url), "agent-browser-linux-x64").is_err());
    }
}
