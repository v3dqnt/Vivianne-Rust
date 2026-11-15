//! The native Vivianne app.
//!
//! Set `VIV_MODEL` plus a provider key to chat for real (see
//! `vivianne_providers::env`), or `VIV_DEMO=1` for an offline canned reply.

mod chat;
mod demo;
mod theme;

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use tokio::runtime::Runtime;
use vivianne_providers::{Provider, env};
use vivianne_runtime::{Agent, CODING_SYSTEM_PROMPT};
use vivianne_tools::Toolbox;

use chat::{ChatSession, ChatView};

fn main() {
    // The agent and its HTTP calls need tokio; the UI runs on GPUI's own executor.
    let runtime = match Runtime::new() {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => {
            eprintln!("vivianne: cannot start the async runtime: {error}");
            std::process::exit(1);
        }
    };
    let (session, model) = match build_session(runtime) {
        Ok((session, model)) => (Ok(session), model),
        Err(problem) => (Err(setup_hint(&problem)), "not configured".to_owned()),
    };

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(cx);
        let bounds = Bounds::centered(None, size(px(1000.0), px(560.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Vivianne".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            let chat = cx.new(|cx| ChatView::new(session, model, window, cx));
            window.focus(&chat.focus_handle(cx), cx);
            chat
        });
        if let Err(error) = opened {
            eprintln!("vivianne: cannot open the window: {error}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}

fn build_session(runtime: Arc<Runtime>) -> Result<(ChatSession, String), String> {
    let (provider, model): (Arc<dyn Provider>, String) = if std::env::var_os("VIV_DEMO").is_some() {
        (Arc::new(demo::Demo), "demo".to_owned())
    } else {
        (env::provider()?, env::model()?)
    };
    let workdir = std::env::current_dir().map_err(|e| e.to_string())?;
    let agent = Agent::new(
        provider,
        &model,
        Some(CODING_SYSTEM_PROMPT.to_owned()),
        Toolbox::coding(&workdir),
    );
    Ok((ChatSession::new(agent, runtime), model))
}

fn setup_hint(problem: &str) -> String {
    format!(
        "Not set up: {problem}. Set VIV_MODEL and a provider key, or VIV_DEMO=1 to try it offline."
    )
}
