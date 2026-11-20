//! Installing a coding agent where Vivianne owns it.
//!
//! `npx -y <package>` fetches an agent on its first run and says nothing while it does. The Claude Code
//! agent bundles the Claude Code binary, so that first run is a very large download and looks exactly like
//! a hang. Here the package is installed once into `~/.viv/agents/<id>/`, with its progress shown, a receipt
//! records where its program is, and from then on the agent starts as `node <script>` with no `npx` and no
//! network.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use vivianne_ai::paths::{viv_home_path, write_json_atomic};

use super::conn::{quiet, trace};
use super::registry::{Harness, mark_ready};

/// The Claude Code agent alone is a few hundred megabytes.
const INSTALL_LIMIT: Duration = Duration::from_mins(20);
const PROGRESS_EVERY: Duration = Duration::from_millis(1500);
/// How many files to count when sizing an install folder.
const SIZE_SCAN_LIMIT: usize = 50_000;

/// What an install left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// `name@version`, as the registry named it.
    pub package: String,
    /// The program to run with `node`, absolute.
    pub script: String,
}

#[must_use]
pub fn agent_dir(id: &str) -> PathBuf {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    viv_home_path(&["agents", &safe])
}

/// `@scope/name@1.2.3` as (`@scope/name`, `Some("1.2.3")`); a name with no version has none.
#[must_use]
pub fn split_package(spec: &str) -> (String, Option<String>) {
    let at = spec.char_indices().skip(1).find(|(_, c)| *c == '@').map(|(i, _)| i);
    match at {
        Some(i) => (spec[..i].to_owned(), Some(spec[i + 1..].to_owned())),
        None => (spec.to_owned(), None),
    }
}

/// The script a package's `bin` names: its string form, or the entry named like the package (without its
/// scope), or the first one.
#[must_use]
pub fn bin_script(package_json: &Value, package_name: &str) -> Option<String> {
    let bin = package_json.get("bin")?;
    if let Some(script) = bin.as_str() {
        return Some(script.to_owned());
    }
    let map = bin.as_object()?;
    let short = package_name.rsplit('/').next().unwrap_or(package_name);
    map.get(short).or_else(|| map.values().next()).and_then(Value::as_str).map(str::to_owned)
}

/// The receipt for `id`, if it matches the package the registry lists now (a newer registry version needs a
/// fresh install) and its program is still there.
#[must_use]
pub fn receipt_for(id: &str, package: &str) -> Option<Receipt> {
    let text = std::fs::read_to_string(agent_dir(id).join("receipt.json")).ok()?;
    let receipt: Receipt = serde_json::from_str(&text).ok()?;
    (receipt.package == package && Path::new(&receipt.script).is_file()).then_some(receipt)
}

fn write_receipt(id: &str, receipt: &Receipt) -> Result<(), String> {
    write_json_atomic(&agent_dir(id).join("receipt.json"), receipt)
}

/// The program of an installed package: `node_modules/<name>/<bin script>` in `dir`.
pub fn installed_script(dir: &Path, package: &str) -> Result<PathBuf, String> {
    let (name, _) = split_package(package);
    let root = dir.join("node_modules").join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
    let text = std::fs::read_to_string(root.join("package.json"))
        .map_err(|e| format!("The package didn't install: no package.json in {} ({e})", root.display()))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("The package's package.json is invalid: {e}"))?;
    let script = bin_script(&json, &name).ok_or("The package doesn't say which program to run (no `bin`).")?;
    let path = root.join(script.trim_start_matches("./"));
    path.is_file().then_some(path).ok_or_else(|| "The package's program is missing after the install.".to_owned())
}

/// What an npm progress line says, in words, or nothing for a line that isn't progress. With
/// `--loglevel http` npm prints one line per request.
#[must_use]
pub fn progress_line(line: &str) -> Option<String> {
    let l = line.trim();
    if l.is_empty() || l.contains("npm warn") {
        return None;
    }
    if let Some(rest) = l.strip_prefix("npm http fetch ") {
        let mut parts = rest.split_whitespace();
        let (_method, status, url) = (parts.next()?, parts.next()?, parts.next()?);
        let file = url.rsplit('/').next().unwrap_or(url);
        return Some(if std::path::Path::new(file).extension().is_some_and(|e| e.eq_ignore_ascii_case("tgz")) { format!("Downloaded {file}") } else { format!("Looked up {} ({status})", file.trim_end_matches(".json")) });
    }
    if let Some(rest) = l.strip_prefix("npm http cache ") {
        let url = rest.split_whitespace().next().unwrap_or_default();
        let file = url.rsplit('/').next().unwrap_or(url);
        return Some(format!("Using the copy npm already had of {}", file.trim_end_matches(".tgz")));
    }
    if l.starts_with("npm error") || l.starts_with("npm ERR") {
        return Some(l.trim_start_matches("npm error").trim_start_matches("npm ERR!").trim().to_owned());
    }
    None
}

/// Total size of a folder in bytes, counting at most `limit` entries (a cheap size for a progress line).
#[must_use]
pub fn dir_bytes(dir: &Path, limit: usize) -> u64 {
    let (mut total, mut seen) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > limit {
                return total;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// Where an install is: the last thing npm did and how much is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProgress {
    pub line: String,
    pub bytes: u64,
}

/// Installs an npm agent into `~/.viv/agents/<id>` with `npm install --prefix`, telling `progress` what npm
/// is doing as it goes.
pub(super) async fn install_managed(harness: &Harness, package: &str, progress: &(dyn Fn(InstallProgress) + Sync)) -> Result<String, String> {
    let npm = which::which("npm").map_err(|_| format!("{} needs npm, which isn't on this computer's PATH. Install Node.js first.", harness.name))?;
    let dir = agent_dir(&harness.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut command = quiet(&npm);
    command
        .args(["install", "--prefix"])
        .arg(&dir)
        .args(["--no-audit", "--no-fund", "--loglevel=http", package])
        .envs(&harness.env)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    trace(&harness.name, format!("installing {package} into {}", dir.display()));
    let mut child = command.spawn().map_err(|e| format!("Couldn't run npm: {e}"))?;
    let stderr = child.stderr.take().ok_or("npm has no error output")?;
    let say = |line: &str| progress(InstallProgress { line: line.to_owned(), bytes: dir_bytes(&dir, SIZE_SCAN_LIMIT) });
    say("Starting npm…");

    let mut lines = BufReader::new(stderr).lines();
    let mut tick = tokio::time::interval(PROGRESS_EVERY);
    let mut last = String::from("Starting npm…");
    let mut errors: Vec<String> = Vec::new();
    let work = async {
        loop {
            tokio::select! {
                next = lines.next_line() => match next {
                    Ok(Some(line)) => {
                        trace(&harness.name, format!("npm: {line}"));
                        if line.contains("npm error") || line.contains("npm ERR") {
                            errors.push(line.clone());
                        }
                        if let Some(update) = progress_line(&line) {
                            last = update;
                        }
                    }
                    _ => break,
                },
                _ = tick.tick() => say(&last),
            }
        }
        child.wait().await
    };
    let status = tokio::time::timeout(INSTALL_LIMIT, work)
        .await
        .map_err(|_| format!("{} took over 20 minutes to download.", harness.name))?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        let tail = errors[errors.len().saturating_sub(6)..].join("\n");
        return Err(format!("npm couldn't install {}:\n{}", harness.name, if tail.is_empty() { status.to_string() } else { tail }));
    }
    let script = installed_script(&dir, package)?;
    write_receipt(&harness.id, &Receipt { package: package.to_owned(), script: script.to_string_lossy().into_owned() })?;
    mark_ready(&harness.id);
    say("Done");
    trace(&harness.name, format!("installed: {}", script.display()));
    Ok(format!("{} is installed.", harness.name))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_package_spec_splits_into_name_and_version() {
        assert_eq!(split_package("@agentclientprotocol/claude-agent-acp@0.100.0"), ("@agentclientprotocol/claude-agent-acp".into(), Some("0.100.0".into())));
        assert_eq!(split_package("cline@1.2.3"), ("cline".into(), Some("1.2.3".into())));
        assert_eq!(split_package("@scope/pkg"), ("@scope/pkg".into(), None));
        assert_eq!(split_package("pkg"), ("pkg".into(), None));
    }

    #[test]
    fn the_program_is_found_in_either_form_of_bin() {
        assert_eq!(bin_script(&json!({ "bin": "dist/index.js" }), "x").as_deref(), Some("dist/index.js"));
        assert_eq!(
            bin_script(&json!({ "bin": { "claude-agent-acp": "dist/a.js", "other": "dist/b.js" } }), "@agentclientprotocol/claude-agent-acp").as_deref(),
            Some("dist/a.js")
        );
        assert_eq!(bin_script(&json!({ "bin": { "tool": "t.js" } }), "different-name").as_deref(), Some("t.js"), "no name match: the first");
        assert_eq!(bin_script(&json!({}), "x"), None);
        assert_eq!(bin_script(&json!({ "bin": 3 }), "x"), None);
    }

    #[test]
    fn npm_lines_become_progress_and_noise_is_dropped() {
        assert_eq!(
            progress_line("npm http fetch GET 200 https://registry.npmjs.org/@a/b/-/b-0.3.284.tgz 41233ms (cache miss)").as_deref(),
            Some("Downloaded b-0.3.284.tgz")
        );
        assert_eq!(progress_line("npm http fetch GET 200 https://registry.npmjs.org/zod 120ms").as_deref(), Some("Looked up zod (200)"));
        assert_eq!(
            progress_line("npm http cache content-type@https://registry.npmjs.org/content-type/-/content-type-2.1.0.tgz 0ms (cache hit)").as_deref(),
            Some("Using the copy npm already had of content-type-2.1.0")
        );
        assert_eq!(progress_line("npm warn Unknown env config"), None);
        assert_eq!(progress_line("npm error code E404").as_deref(), Some("code E404"));
        assert_eq!(progress_line("   "), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-acp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_installed_package_is_found_through_its_package_json() {
        let dir = scratch("installed");
        let pkg = dir.join("node_modules").join("@scope").join("agent");
        std::fs::create_dir_all(pkg.join("dist")).unwrap_or_default();
        std::fs::write(pkg.join("package.json"), r#"{"name":"@scope/agent","bin":{"agent":"./dist/run.js"}}"#).unwrap_or_default();
        std::fs::write(pkg.join("dist").join("run.js"), "// agent").unwrap_or_default();
        let script = installed_script(&dir, "@scope/agent@1.0.0");
        assert!(script.as_ref().is_ok_and(|s| s.ends_with(Path::new("dist").join("run.js"))), "{script:?}");
        assert!(dir_bytes(&dir, 1000) >= 8);
        std::fs::remove_file(pkg.join("dist").join("run.js")).unwrap_or_default();
        assert!(installed_script(&dir, "@scope/agent@1.0.0").is_err_and(|e| e.contains("missing")));
        assert!(installed_script(&dir.join("nowhere"), "@scope/agent").is_err_and(|e| e.contains("didn't install")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_receipt_counts_only_for_the_package_listed_now_and_while_its_program_exists() {
        let id = format!("test-receipt-{}", std::process::id());
        let dir = agent_dir(&id);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let script = dir.join("run.js");
        std::fs::write(&script, "// x").unwrap_or_default();
        let wrote = write_receipt(&id, &Receipt { package: "a@1.0.0".into(), script: script.to_string_lossy().into_owned() });
        assert!(wrote.is_ok());
        assert!(receipt_for(&id, "a@1.0.0").is_some());
        assert!(receipt_for(&id, "a@1.1.0").is_none(), "a newer registry version needs a fresh install");
        std::fs::remove_file(&script).unwrap_or_default();
        assert!(receipt_for(&id, "a@1.0.0").is_none(), "the program is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
