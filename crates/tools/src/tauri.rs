#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the two Tauri calls the copied code makes. The new app has no Tauri.

pub mod async_runtime {
    pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future)
    }
}
