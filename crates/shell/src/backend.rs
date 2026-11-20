//! The app's services that live outside the window (keys, ChatGPT sign-in), opened once at start, and a
//! way for the UI to run async work on them and get the answer back on the UI thread.

use std::future::Future;
use std::sync::{Arc, OnceLock};

use gpui_kit::App;
use tokio::runtime::{Handle, Runtime};
use vivianne_ai::events::Events;
use vivianne_ai::inference::usage_store;
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
    // Usage is kept in memory and written on a timer, so the last of a session is not lost to a crash.
    runtime.spawn(usage_store::flush_periodically());
    let _ = BACKEND.set(Backend { handle: runtime.handle().clone(), hub, acp: Arc::new(Acp::new()) });
}

#[must_use]
pub fn get() -> Option<&'static Backend> {
    BACKEND.get()
}

/// Starts a dev server. It is a child process watched by tasks on the async runtime, so it is started with that
/// runtime entered.
pub fn start_dev_server(id: &str, cwd: &str, command: &str) -> Result<(), String> {
    let backend = get().ok_or("The background services did not start.")?;
    let _entered = backend.handle.enter();
    vivianne_tools::dev_server::DEV_SERVERS.start(id, cwd, command)
}

/// Writes the usage still in memory when the app quits. GPUI waits for this a moment (a fraction of a second),
/// and writing a few small files takes far less.
pub fn flush_on_quit(cx: &mut App) {
    cx.on_app_quit(|_cx| async {
        // A dev server started from the app would otherwise outlive it.
        vivianne_tools::dev_server::DEV_SERVERS.stop_all();
        if let Some(flushed) = spawn(usage_store::flush_all()) {
            let _ = flushed.await;
        }
    })
    .detach();
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

/// Starts `work` on the async runtime and gives back where its answer will arrive, for code that waits for it
/// inside its own async task. `None` if the services never opened.
pub fn spawn<T: Send + 'static>(work: impl Future<Output = T> + Send + 'static) -> Option<futures::channel::oneshot::Receiver<T>> {
    let backend = get()?;
    let (tx, rx) = futures::channel::oneshot::channel();
    backend.handle.spawn(async move {
        let _ = tx.send(work.await);
    });
    Some(rx)
}

/// Notes how a coding agent's sign-in ended: which agent and the outcome, nothing else.
pub fn note_sign_in(agent: &str, result: &Result<vivianne_providers::acp::AuthState, String>) {
    use vivianne_providers::acp::AuthState;
    let outcome = match result {
        Ok(AuthState::SignedIn) => "signed_in",
        Ok(AuthState::NeedsSignIn(_)) => "needs_sign_in",
        Ok(AuthState::Failed(_)) | Err(_) => "failed",
    };
    vivianne_telemetry::track("agent.sign_in", serde_json::json!({ "agent": agent, "outcome": outcome }));
}
