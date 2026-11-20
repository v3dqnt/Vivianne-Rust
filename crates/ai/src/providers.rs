//! The provider keys the user has: saving and removing them, extra keys that take over when one is
//! limited, and the ChatGPT plan sign-in. This is what Settings → Providers talks to.
//!
//! Keys live where the old app kept them: the OS keychain (service `vivianne`) when there is one,
//! otherwise `~/.viv/agent/auth.json`. Extra keys sit beside them (`pool:<provider>` in the keychain,
//! `~/.viv/agent/credential_pool_secrets.json` otherwise); their cooldowns and counts are in
//! `~/.viv/agent/credential_pool_state.json`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ai::auth::{
    ApiKeyCredential, AuthState, Credential, CredentialPool, FilePoolSecrets, InMemoryPoolSecrets, KeychainPoolSecrets,
    PoolEntryInfo, PoolRuntimeStore, PoolSecretsStore, RotationStrategy,
};
use crate::ai::chatgpt::{self, ChatGptStatus};
use crate::ai::env_api_keys::get_env_api_key;
use crate::ai::keychain_store::KeychainCredentialStore;
use crate::events::Events;
use crate::paths::viv_home_path;

/// The label of the key saved in the provider's main slot.
pub const PRIMARY_LABEL: &str = "Primary key";

/// Where one provider's key came from, for showing the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// Saved in Vivianne.
    Saved,
    /// An environment variable.
    Environment,
    None,
}

pub struct Hub {
    auth: Arc<AuthState>,
    pool: Arc<CredentialPool>,
    events: Events,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").finish_non_exhaustive()
    }
}

impl Hub {
    /// Opens the key stores and registers them on `events` so the provider layer finds them.
    pub async fn start(events: &Events) -> Arc<Self> {
        let keychain = tokio::task::spawn_blocking(KeychainCredentialStore::is_available).await.unwrap_or(false);
        let (auth, pool) = tokio::join!(open_auth(keychain), open_pool(keychain));
        let (auth, pool) = (Arc::new(auth), Arc::new(pool));
        crate::ai::auth::set_global(auth.clone());
        events.manage(auth.clone());
        Arc::new(Self { auth, pool, events: events.clone() })
    }

    // ── Keys ────────────────────────────────────────────────────────────────

    /// Where each provider's key comes from, asked for all of them at once.
    pub async fn sources(&self, providers: &[String]) -> HashMap<String, KeySource> {
        let asks = providers.iter().map(|id| async move { (id.clone(), self.source(id).await) });
        futures::future::join_all(asks).await.into_iter().collect()
    }

    pub async fn source(&self, provider: &str) -> KeySource {
        if matches!(self.auth.store.read(provider).await, Ok(Some(_))) {
            KeySource::Saved
        } else if get_env_api_key(provider, None).is_some() {
            KeySource::Environment
        } else {
            KeySource::None
        }
    }

    pub async fn save_key(&self, provider: &str, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() {
            return Err("The key is empty.".to_owned());
        }
        let credential = Credential::ApiKey(ApiKeyCredential { kind: "api_key".to_owned(), key: Some(key.to_owned()), env: None });
        self.auth.store.modify(provider, Box::new(move |_| Box::pin(async move { Some(credential) }))).await?;
        let _ = self.events.emit("tool-defs-invalidated", ());
        Ok(())
    }

    pub async fn delete_key(&self, provider: &str) -> Result<(), String> {
        self.auth.store.delete(provider).await?;
        let _ = self.events.emit("tool-defs-invalidated", ());
        Ok(())
    }

    // ── Extra keys ──────────────────────────────────────────────────────────

    /// The provider's keys (main first) and how they are used in turn.
    pub async fn pool(&self, provider: &str) -> (Vec<PoolEntryInfo>, RotationStrategy) {
        let primary = matches!(self.auth.store.read(provider).await, Ok(Some(_))).then(|| PRIMARY_LABEL.to_owned());
        let entries = self.pool.list_for_ui(provider, primary).await;
        (entries, self.pool.runtime.strategy(provider).await)
    }

    pub async fn add_pool_key(&self, provider: &str, label: &str, key: &str) -> Result<(), String> {
        let (label, key) = (label.trim(), key.trim());
        if key.is_empty() {
            return Err("The key is empty.".to_owned());
        }
        let label = if label.is_empty() { "Extra key" } else { label };
        self.pool.secrets.add(provider, label.to_owned(), key.to_owned()).await.map(|_| ())
    }

    pub async fn remove_pool_key(&self, provider: &str, credential_id: &str) -> Result<(), String> {
        self.pool.secrets.remove(provider, credential_id).await?;
        self.pool.runtime.forget(provider, credential_id).await;
        Ok(())
    }

    pub async fn set_strategy(&self, provider: &str, strategy: RotationStrategy) {
        self.pool.runtime.set_strategy(provider, strategy).await;
    }

    pub async fn reset_cooldowns(&self, provider: &str) {
        self.pool.runtime.reset_cooldowns(provider).await;
    }

    // ── ChatGPT plan ────────────────────────────────────────────────────────

    pub async fn chatgpt_status(&self) -> ChatGptStatus {
        chatgpt::status(&self.events).await
    }

    pub async fn chatgpt_sign_in(&self) -> Result<ChatGptStatus, String> {
        chatgpt::sign_in(&self.events).await
    }

    pub async fn chatgpt_sign_out(&self) -> Result<ChatGptStatus, String> {
        chatgpt::sign_out(&self.events).await
    }

    pub fn chatgpt_manage_usage(&self) -> Result<(), String> {
        chatgpt::manage_usage()
    }
}

async fn open_auth(keychain: bool) -> AuthState {
    if keychain {
        return AuthState::with_keychain_store();
    }
    // No keychain here: keys go in a file the way the old app did, unencrypted.
    AuthState::with_file_store(viv_home_path(&["agent", "auth.json"])).await.unwrap_or_default()
}

async fn open_pool(keychain: bool) -> CredentialPool {
    let runtime = Arc::new(PoolRuntimeStore::new(viv_home_path(&["agent", "credential_pool_state.json"])).await);
    let secrets: Arc<dyn PoolSecretsStore> = if keychain {
        Arc::new(KeychainPoolSecrets::new())
    } else {
        match FilePoolSecrets::new(viv_home_path(&["agent", "credential_pool_secrets.json"])).await {
            Ok(store) => Arc::new(store),
            // Extra keys then last only this session; better said than silently lost, but the page has no
            // place to say it yet.
            Err(_) => Arc::new(InMemoryPoolSecrets::new()),
        }
    };
    CredentialPool { secrets, runtime }
}
