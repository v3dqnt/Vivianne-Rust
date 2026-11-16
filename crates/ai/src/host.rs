//! What the ai layer needs from the app around it, in one place.
//!
//! The old app reached into its agent runtime for these (settings, key lookup, downloads, routing,
//! memory). The new app keeps them behind this small module so the copied layer has one seam to
//! the host instead of many.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use crate::ai::auth::{wait_for_state, Credential};
use crate::ai::env_api_keys::get_env_api_key;
use crate::events::Events;
use crate::paths::viv_home;

// ── The model a background call is made on ──────────────────────────────────

/// A chosen model, as the rest of the app hands it over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub reasoning: bool,
    pub max_tokens: u32,
    pub context_window: u32,
}

impl Default for ModelRef {
    fn default() -> Self {
        Self {
            id: "unknown".to_owned(),
            name: "unknown".to_owned(),
            api: "unknown".to_owned(),
            provider: "unknown".to_owned(),
            base_url: None,
            reasoning: false,
            max_tokens: 0,
            context_window: 0,
        }
    }
}

/// The model background work (summaries, memory upkeep) should use. Cost-aware routing is not part
/// of the new app yet, so this is the model it was given.
#[must_use]
pub fn background_model(model: &ModelRef) -> ModelRef {
    model.clone()
}

// ── Settings ────────────────────────────────────────────────────────────────

fn settings_path() -> Result<PathBuf, String> {
    Ok(viv_home()?.join("agent").join("settings.json"))
}

/// `~/.viv/agent/settings.json`, or an empty object when it is missing or unreadable.
pub async fn read_settings_json() -> Value {
    let Ok(path) = settings_path() else { return json!({}) };
    let Ok(text) = tokio::fs::read_to_string(path).await else { return json!({}) };
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

/// Replace `~/.viv/agent/settings.json`.
pub async fn save_settings_json(settings: &Value) -> Result<(), String> {
    let path = settings_path()?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    tokio::fs::write(path, text).await.map_err(|e| e.to_string())
}

// ── Provider keys ───────────────────────────────────────────────────────────

fn key_of(credential: Credential) -> Option<String> {
    match credential {
        Credential::ApiKey(key) => key.key,
        Credential::OAuth(oauth) => Some(oauth.access_token),
    }
}

/// The key for `provider`: the stored credential first, then the environment.
pub async fn resolve_provider_api_key(app: &Events, provider: &str) -> Option<String> {
    let auth = wait_for_state(app).await;
    let stored = auth.store.read(provider).await.ok().flatten().and_then(key_of);
    stored.or_else(|| get_env_api_key(provider, None))
}

// ── Memory ──────────────────────────────────────────────────────────────────

/// Event telling the memory store to re-embed everything (the embedding model changed).
pub const REEMBED_EVENT: &str = "memory-reembed";

/// Ask the memory store, when the app has one, to re-embed what it holds.
pub fn reembed_everywhere(app: &Events) {
    let _ = app.emit(REEMBED_EVENT, ());
}

// ── Downloads ───────────────────────────────────────────────────────────────

pub mod download {
    //! Resumable model downloads with progress events.

    use super::{AsyncWriteExt, Events, HashSet, LazyLock, Mutex, Path, PathBuf, PoisonError, Serialize, StreamExt};

    static CANCELLED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

    fn client() -> &'static reqwest::Client {
        static CLIENT: LazyLock<reqwest::Client> =
            LazyLock::new(|| reqwest::Client::builder().user_agent("Vivianne").build().expect("http client"));
        &CLIENT
    }

    fn cancelled() -> std::sync::MutexGuard<'static, HashSet<String>> {
        CANCELLED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Progress<'a> {
        file: &'a str,
        received: u64,
        total: u64,
    }

    /// Stop the download of `file` at its next chunk.
    pub fn cancel(file: &str) {
        cancelled().insert(file.to_owned());
    }

    /// Download `url` into `<dir>/<file>`, resuming a `.part` left by an earlier try. Progress goes
    /// out as `event`.
    pub async fn fetch_to(app: &Events, url: &str, dir: &Path, file: &str, event: &str) -> Result<PathBuf, String> {
        cancelled().remove(file);
        let part = dir.join(format!("{file}.part"));
        let already = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
        let response = start_request(url, already).await?;

        // A server that ignores Range sends the whole file again.
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let start = if resumed { already } else { 0 };
        let total = start + response.content_length().unwrap_or(0);
        let mut out = open_part(&part, resumed).await?;

        let received = copy_body(app, response, &mut out, (file, event), (start, total)).await?;
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);

        let dest = dir.join(file);
        tokio::fs::rename(&part, &dest).await.map_err(|e| format!("Couldn't finish the model file: {e}"))?;
        let _ = app.emit(event, Progress { file, received, total: received });
        Ok(dest)
    }

    async fn start_request(url: &str, resume_from: u64) -> Result<reqwest::Response, String> {
        let mut request = client().get(url);
        if resume_from > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
        }
        let response = request.send().await.map_err(|e| format!("Couldn't start the download: {e}"))?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!("Download failed: HTTP {}", response.status()))
        }
    }

    async fn open_part(part: &Path, resumed: bool) -> Result<tokio::fs::File, String> {
        let opened = if resumed {
            tokio::fs::OpenOptions::new().append(true).open(part).await
        } else {
            tokio::fs::File::create(part).await
        };
        opened.map_err(|e| format!("Couldn't write the model file: {e}"))
    }

    async fn copy_body(
        app: &Events,
        response: reqwest::Response,
        out: &mut tokio::fs::File,
        (file, event): (&str, &str),
        (start, total): (u64, u64),
    ) -> Result<u64, String> {
        let mut received = start;
        let mut last_report = std::time::Instant::now();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            if cancelled().remove(file) {
                return Err("Cancelled".to_owned());
            }
            let chunk = chunk.map_err(|e| format!("The download was interrupted: {e}. Try again to resume."))?;
            out.write_all(&chunk).await.map_err(|e| format!("Couldn't write the model file: {e}"))?;
            received += chunk.len() as u64;
            if last_report.elapsed().as_millis() > 200 {
                last_report = std::time::Instant::now();
                let _ = app.emit(event, Progress { file, received, total });
            }
        }
        Ok(received)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cancelling_marks_the_file_until_a_download_clears_it() {
            cancel("host-test.bin");
            assert!(cancelled().contains("host-test.bin"));
            cancelled().remove("host-test.bin");
        }

        #[tokio::test]
        async fn a_bad_url_is_an_error_not_a_panic() {
            let dir = std::env::temp_dir();
            let err = fetch_to(&Events::new(), "http://127.0.0.1:1/none", &dir, "host-test.bin", "x").await;
            assert!(err.unwrap_err().starts_with("Couldn't start the download"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_work_keeps_the_given_model() {
        let model = ModelRef { id: "m".into(), provider: "p".into(), ..ModelRef::default() };
        assert_eq!(background_model(&model).id, "m");
    }
}
