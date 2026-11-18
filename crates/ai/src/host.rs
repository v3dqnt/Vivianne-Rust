//! What the ai layer needs from the app around it, in one place.
//!
//! The old app reached into its agent runtime for these (settings, key lookup, downloads, routing,
//! memory). The new app keeps them behind this small module so the copied layer has one seam to
//! the host instead of many.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use crate::ai::auth::{wait_for_state, AuthState, Credential, CredentialStore, DefaultAuthContext, ModifyFuture};
use crate::ai::env_api_keys::get_env_api_key;
use crate::ai::keychain_store::KeychainCredentialStore;
use crate::ai::model_discovery::{list_cached_or_fresh, DiscoveredModel};
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

/// The model background work (summaries, memory upkeep) should use. This is the model it was given;
/// `inference::routing::route` is the async, cost-aware choice for callers that can await.
#[must_use]
pub fn background_model(model: &ModelRef) -> ModelRef {
    model.clone()
}

// ── Settings ────────────────────────────────────────────────────────────────

fn settings_path() -> Result<PathBuf, String> {
    Ok(viv_home()?.join("agent").join("settings.json"))
}

/// `~/.viv/agent/settings.json`, or an empty object when it is missing or unreadable.
#[must_use]
pub fn settings_blocking() -> Value {
    let Ok(path) = settings_path() else { return json!({}) };
    let Ok(text) = std::fs::read_to_string(path) else { return json!({}) };
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

/// [`settings_blocking`] for async callers.
pub async fn read_settings_json() -> Value {
    tokio::task::spawn_blocking(settings_blocking).await.unwrap_or_else(|_| json!({}))
}

/// Record `provider`/`model` as the last model used, in the settings file the old app reads it from.
pub fn remember_last_model(provider: &str, model: &str) -> Result<(), String> {
    let mut settings = settings_blocking();
    let Some(object) = settings.as_object_mut() else { return Err("settings.json is not an object".to_owned()) };
    object.insert("lastModel".to_owned(), json!({ "id": model, "provider": provider }));
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(settings_path()?, text).map_err(|e| e.to_string())
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

fn key_in(credential: Credential) -> Option<String> {
    key_of(credential).filter(|key| !key.trim().is_empty())
}

/// A key the old Vivianne saved for `provider`: the OS keychain (entry `vivianne`/`provider`) first, then
/// `~/.viv/agent/auth.json`. Reads without an async runtime.
#[must_use]
pub fn stored_api_key(provider: &str) -> Option<String> {
    std::iter::once(provider).chain(provider_alias(provider)).find_map(|id| {
        let from_keychain = KeychainCredentialStore::read_blocking(id).ok().flatten();
        from_keychain.or_else(|| credential_from_auth_file(id)).and_then(key_in)
    })
}

/// Another id the old app may have saved the same provider's key under.
fn provider_alias(provider: &str) -> Option<&'static str> {
    match provider {
        "nvidia" => Some("nvidia-nim"),
        "nvidia-nim" => Some("nvidia"),
        _ => None,
    }
}

fn credential_from_auth_file(provider: &str) -> Option<Credential> {
    let path = viv_home().ok()?.join("agent").join("auth.json");
    let text = std::fs::read_to_string(path).ok()?;
    let mut credentials: HashMap<String, Credential> = serde_json::from_str(&text).ok()?;
    credentials.remove(provider)
}

/// The credentials the old app saved (OS keychain, then `~/.viv/agent/auth.json`), read-only: this app
/// reads them, the old app's settings screen is where they are changed.
#[derive(Default)]
pub struct SavedCredentials {
    keychain: KeychainCredentialStore,
}

#[async_trait::async_trait]
impl CredentialStore for SavedCredentials {
    async fn read(&self, provider_id: &str) -> Result<Option<Credential>, String> {
        for id in std::iter::once(provider_id).chain(provider_alias(provider_id)) {
            if let Some(credential) = self.keychain.read(id).await.ok().flatten() {
                return Ok(Some(credential));
            }
            if let Some(credential) = credential_from_auth_file(id) {
                return Ok(Some(credential));
            }
        }
        Ok(None)
    }

    async fn modify(
        &self,
        _provider_id: &str,
        _change: Box<dyn FnOnce(Option<Credential>) -> ModifyFuture + Send>,
    ) -> Result<Option<Credential>, String> {
        Err("saved credentials are read-only here".to_owned())
    }

    async fn delete(&self, _provider_id: &str) -> Result<(), String> {
        Err("saved credentials are read-only here".to_owned())
    }
}

/// The models every provider with a saved or environment key offers. The last list is reused while it is
/// fresh (`~/.viv/models-cache.json`); `refresh` asks the providers again.
pub async fn discover_models(refresh: bool) -> Vec<DiscoveredModel> {
    let auth = AuthState {
        store: std::sync::Arc::new(SavedCredentials::default()),
        auth_context: std::sync::Arc::new(DefaultAuthContext),
    };
    list_cached_or_fresh(&auth, refresh).await.0
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
    use crate::ai::auth::ApiKeyCredential;

    fn api_key(key: Option<&str>) -> Credential {
        Credential::ApiKey(ApiKeyCredential { kind: "api_key".into(), key: key.map(str::to_owned), env: None })
    }

    #[test]
    fn a_blank_stored_key_is_no_key() {
        assert_eq!(key_in(api_key(Some("sk-1"))).as_deref(), Some("sk-1"));
        assert_eq!(key_in(api_key(Some("  "))), None);
        assert_eq!(key_in(api_key(None)), None);
    }

    #[test]
    fn the_old_apps_auth_file_format_parses() {
        let file = r#"{"openai": {"type": "api_key", "key": "sk-1"}, "x": {"type": "o_auth", "accessToken": "t", "refreshToken": null, "expires": 1}}"#;
        let parsed: HashMap<String, Credential> = serde_json::from_str(file).unwrap();
        assert_eq!(parsed.get("openai").cloned().and_then(key_in).as_deref(), Some("sk-1"));
        assert_eq!(parsed.get("x").cloned().and_then(key_in).as_deref(), Some("t"));
    }

    #[test]
    fn the_two_nvidia_ids_find_each_others_keys() {
        assert_eq!(provider_alias("nvidia"), Some("nvidia-nim"));
        assert_eq!(provider_alias("nvidia-nim"), Some("nvidia"));
        assert_eq!(provider_alias("openai"), None);
    }

    #[tokio::test]
    async fn saved_credentials_cannot_be_written() {
        let store = SavedCredentials::default();
        assert!(store.delete("openai").await.is_err());
    }

    #[test]
    fn background_work_keeps_the_given_model() {
        let model = ModelRef { id: "m".into(), provider: "p".into(), ..ModelRef::default() };
        assert_eq!(background_model(&model).id, "m");
    }
}
