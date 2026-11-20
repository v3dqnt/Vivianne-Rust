//! The Settings dialog: a page list on the left, the page on the right.
//! Pages follow the old app. Values live in memory only for now (see `values`).

mod defaults;
mod pages;
mod provider_state;
mod usage_data;
mod values;
mod widgets;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::setting::Settings;
use gpui_kit::prelude::*;
use gpui_kit::{App, Window, div, px};

use values::Values;

const DIALOG_WIDTH: f32 = 980.0;
const DIALOG_HEIGHT: f32 = 560.0;
const SIDEBAR_WIDTH: f32 = 220.0;

/// Open the Settings dialog over `window`.
pub fn open(window: &mut Window, cx: &mut App) {
    // Created once per opening, so edits survive the dialog being rebuilt.
    let values = Rc::new(RefCell::new(Values::default()));
    defaults::fill(&mut values.borrow_mut());
    // The dialog opens on the first page, Account; the sidebar does not say when another is picked.
    vivianne_telemetry::track("settings.open", serde_json::json!({ "page": "account" }));
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let settings = Settings::new("settings")
            .sidebar_width(px(SIDEBAR_WIDTH))
            .pages(pages::all(&values));
        dialog
            .title("Settings")
            .w(px(DIALOG_WIDTH))
            .child(div().h(px(DIALOG_HEIGHT)).child(settings))
    });
}
