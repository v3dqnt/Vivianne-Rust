//! The Settings dialog: a page list on the left, the page on the right.
//! Pages follow the old app. Values live in memory only for now (see `values`).

mod crop;
mod defaults;
pub mod logos;
mod pages;
mod profile;
mod provider_state;
mod usage_data;
mod values;
mod widgets;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::group_box::GroupBoxVariant;
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
    // The icons of the providers and coding agents come from their sites, once; they appear as they arrive.
    crate::brand::prefetch(cx, crate::brand::known_domains());
    window.open_dialog(cx, move |dialog, window, _cx| {
        // Most of the window, so more fits: at least the old size, at most a comfortable reading width.
        let size = window.viewport_size();
        let width = (f32::from(size.width) - 96.0).clamp(DIALOG_WIDTH, 1400.0);
        let height = (f32::from(size.height) - 96.0).clamp(DIALOG_HEIGHT, 960.0);
        let settings = Settings::new("settings")
            .sidebar_width(px(SIDEBAR_WIDTH))
            .with_group_variant(GroupBoxVariant::Outline)
            .pages(pages::all(&values));
        // No title bar: the sidebar carries the heading, and the sheet runs edge to edge under the close button.
        dialog
            .w(px(width))
            .p_0()
            .bg(crate::theme::color(crate::theme::PANEL))
            .child(
                div()
                    .h(px(height))
                    .rounded(px(crate::theme::RADIUS_PANEL))
                    .overflow_hidden()
                    .child(settings),
            )
    });
}
