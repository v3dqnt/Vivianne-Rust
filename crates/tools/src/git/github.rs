//! The Git tab's window onto GitHub: one REST call for the repository of a folder. These are the user's own
//! clicks, so writes are not gated here (the agent's GitHub tool is another matter). The token comes from the
//! first place that has one: the one saved in Settings, `GITHUB_TOKEN` / `GH_TOKEN`, then the `gh` login.

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use super::git;
use crate::proc::quiet;

const API: &str = "https://api.github.com";

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("vivianne")
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    })
}

/// The `gh` login, asked once per process.
static GH: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();

/// A token: `saved` (from Settings) if there is one, else the environment, else what `gh auth token` says.
pub async fn token(saved: Option<String>) -> Option<String> {
    if let Some(t) = saved.filter(|t| !t.trim().is_empty()) {
        return Some(t);
    }
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(var)
            && !t.trim().is_empty()
        {
            return Some(t.trim().to_owned());
        }
    }
    GH.get_or_init(|| async {
        let gh = which::which("gh").ok()?;
        let out = tokio::time::timeout(
            Duration::from_secs(10),
            quiet(gh)
                .args(["auth", "token"])
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()?
        .ok()?;
        let t = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && !t.is_empty()).then_some(t)
    })
    .await
    .clone()
}

/// `owner/name` from a remote URL, https or ssh.
#[must_use]
pub fn parse_remote(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = [
        "https://github.com/",
        "http://github.com/",
        "git@github.com:",
        "ssh://git@github.com/",
    ]
    .iter()
    .find_map(|prefix| url.strip_prefix(prefix))?;
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?.trim(), parts.next()?.trim());
    (!owner.is_empty() && !name.is_empty()).then(|| format!("{owner}/{name}"))
}

/// The repository `origin` names in `cwd`.
///
/// # Errors
/// There is no origin, or it is not on GitHub.
pub async fn repo(cwd: &str) -> Result<String, String> {
    let url = git(cwd, &["remote", "get-url", "origin"])
        .await
        .map_err(|_| "This folder has no GitHub 'origin' remote.".to_owned())?;
    parse_remote(&url).ok_or_else(|| format!("origin ({url}) isn't a GitHub repository."))
}

/// One REST call. `{repo}` in `path` becomes the folder's `owner/name`. `raw` returns the body as text (job
/// logs).
///
/// # Errors
/// No credentials, a failed request, or GitHub's own refusal with its message.
pub async fn request(
    saved_token: Option<String>,
    cwd: &str,
    path: &str,
    method: &str,
    body: Option<&Value>,
    raw: bool,
) -> Result<Value, String> {
    let token = token(saved_token)
        .await
        .ok_or("No GitHub credentials: sign in with `gh auth login`, set GITHUB_TOKEN, or add a token in Settings.")?;
    let path = if path.contains("{repo}") {
        path.replace("{repo}", &repo(cwd).await?)
    } else {
        path.to_owned()
    };
    let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes())
        .map_err(|e| e.to_string())?;
    let url = if path.starts_with("http") {
        path.clone()
    } else {
        format!("{API}{path}")
    };
    let mut req = client()
        .request(method, &url)
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(b) = body {
        req = req.json(b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("unreadable response: {e}"))?;
    if !(200..300).contains(&status) {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| text.chars().take(300).collect());
        return Err(format!("GitHub returned {status}: {message} ({path})"));
    }
    if raw {
        return Ok(Value::String(text));
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("bad JSON from GitHub: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_in_every_form_name_the_repository() {
        for url in [
            "https://github.com/o/r.git",
            "https://github.com/o/r",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r.git",
            "https://github.com/o/r/",
        ] {
            assert_eq!(parse_remote(url).as_deref(), Some("o/r"), "{url}");
        }
        assert_eq!(parse_remote("https://gitlab.com/o/r.git"), None);
        assert_eq!(parse_remote("https://github.com/o"), None);
    }
}
