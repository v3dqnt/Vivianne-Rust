#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-ins for the parts of the original `ai` module the copied search index
//! reaches into. With them the index uses an embedding key from the environment
//! (`OPENAI_API_KEY`, `NVIDIA_API_KEY`) or falls back to keyword search, exactly
//! as the original does when no key is stored. Stored keys and the offline
//! embedding models come back with the provider and credential layers.

pub mod auth {
    use std::sync::Arc;

    /// A key saved in the credential store. None until that store is ported.
    pub async fn stored_api_key(_provider: &str) -> Option<String> {
        None
    }

    pub struct AuthState;

    impl AuthState {
        pub fn with_keychain_store() -> Self {
            Self
        }
    }

    pub fn set_global(_state: Arc<AuthState>) {}
}

pub mod keychain_store {
    pub struct KeychainCredentialStore;

    impl KeychainCredentialStore {
        pub fn is_available() -> bool {
            false
        }
    }
}

pub mod local_onnx {
    #[derive(Clone, Copy, Debug)]
    pub enum LocalModel {
        Gemma,
        MiniLm,
    }

    #[derive(Clone, Copy, Debug)]
    pub enum Purpose {
        Query,
        Document,
    }

    /// The installed offline embedding model, if any. None until those models are ported.
    pub fn active() -> Option<LocalModel> {
        None
    }

    pub async fn embed_local_batch(
        _inputs: &[String],
        _purpose: Purpose,
    ) -> Result<Option<Vec<Vec<f32>>>, String> {
        Ok(None)
    }
}
