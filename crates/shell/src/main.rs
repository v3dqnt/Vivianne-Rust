//! The native Vivianne app.
//!
//! Set `VIV_MODEL` plus a provider key to chat for real (see
//! `vivianne_providers::env`), or `VIV_DEMO=1` for an offline canned reply.

mod agent_setup;
mod app;
mod assets;
mod backend;
mod brand;
mod characters;
mod chat;
mod demo;
mod git;
mod home;
mod notify;
mod pair;
mod perf;
mod rail;
mod settings;
mod signin;
mod theme;
mod ui_state;

use std::sync::Arc;

use gpui_kit::component::TitleBar;
use gpui_kit::prelude::*;
use gpui_kit::{Bounds, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size};
use tokio::runtime::Runtime;

use app::AppView;

fn main() {
    // First, so a panic anywhere after this leaves a crash record.
    vivianne_telemetry::install_panic_hook(env!("CARGO_PKG_VERSION"));
    // The agent and its HTTP calls need tokio; the UI runs on GPUI's own executor.
    let runtime = match Runtime::new() {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => {
            eprintln!("vivianne: cannot start the async runtime: {error}");
            std::process::exit(1);
        }
    };
    backend::init(&runtime);
    vivianne_telemetry::track(
        "app.start",
        serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS }),
    );
    let workspace = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("vivianne: cannot read the current folder: {error}");
            std::process::exit(1);
        }
    };

    // Kit's icon set (Lucide) is looked up by path, so the app must register it.
    // Every icon of the catalog: the Files tab picks one per file type.
    let assets = assets::Assets::new();
    gpui_kit::application().with_assets(assets).run(move |cx| {
        gpui_kit::init(cx);
        // The typeface is registered before the theme asks for it by name.
        if let Err(why) = cx.text_system().add_fonts(assets::fonts()) {
            vivianne_core::debuglog::log("app", format!("could not load the typeface: {why}"));
        }
        backend::flush_on_quit(cx);
        theme::apply(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(680.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // The window is frosted glass: the desktop behind it is blurred. The header bar and the rail are
            // translucent over it; the page itself paints opaque.
            window_background: WindowBackgroundAppearance::Blurred,
            ..TitleBar::window_options()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| AppView::new(runtime, workspace, window, cx))
        });
        if let Err(error) = opened {
            eprintln!("vivianne: cannot open the window: {error}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
