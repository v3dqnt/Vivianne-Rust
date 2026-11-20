//! The app's services that live outside the window (keys, ChatGPT sign-in), opened once at start, and a
//! way for the UI to run async work on them and get the answer back on the UI thread.

use std::future::Future;
use std::sync::{Arc, OnceLock};

use gpui_kit::App;
use tokio::runtime::{Handle, Runtime};
use vivianne_ai::events::Events;
use vivianne_ai::providers::Hub;
use vivianne_providers::acp::Acp;

pub struct Backend {
    handle: Handle,
    pub hub: Arc<Hub>,
    /// The coding agents that are running.
    pub acp: Arc<Acp>,
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

/// Opens the services. Call once, before the window.
pub fn init(runtime: &Runtime) {
    let events = Events::new();
    let hub = runtime.block_on(Hub::start(&events));
    let _ = BACKEND.set(Backend { handle: runtime.handle().clone(), hub, acp: Arc::new(Acp::new()) });
}

#[must_use]
pub fn get() -> Option<&'static Backend> {
    BACKEND.get()
}

/// Runs `work` on the async runtime and calls `done` with its answer on the UI thread. Nothing happens
/// if the services never opened.
pub fn run<T: Send + 'static>(
    cx: &mut App,
    work: impl Future<Output = T> + Send + 'static,
    done: impl FnOnce(T, &mut App) + 'static,
) {
    let Some(backend) = get() else { return };
    let (tx, rx) = futures::channel::oneshot::channel();
    backend.handle.spawn(async move {
        let _ = tx.send(work.await);
    });
    cx.spawn(async move |cx| {
        if let Ok(answer) = rx.await {
            cx.update(|cx| done(answer, cx));
        }
    })
    .detach();
}
