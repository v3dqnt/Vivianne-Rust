//! The title bar: thread tabs in the middle, Home / rail / Settings on the right.

use gpui_kit::base::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{IconName, Sizable as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Window, div, px};

use super::{AppView, Page};
use crate::settings;
use crate::theme::{self, color};

const TAB_MAX_WIDTH: f32 = 220.0;

pub fn render(app: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    TitleBar::new().child(
        div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(6.0))
            .child(div().pr(px(8.0)).text_size(px(13.0)).child("Vivianne"))
            .child(tabs(app, cx))
            .child(
                // `occlude`: the bar is a window-drag zone; this keeps clicks for us.
                div().occlude().child(
                    Button::new("new-thread")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .on_click(
                            cx.listener(|app, _event, window, cx| app.new_thread(window, cx)),
                        ),
                ),
            )
            .child(div().flex_1())
            .child(actions(app, cx)),
    )
}

fn tabs(app: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .children(app.threads.iter().enumerate().map(|(index, thread)| {
            let chat = thread.chat.read(cx);
            let (title, running) = (chat.title(), chat.is_running());
            let selected = app.page == Page::Thread(index);
            div()
                .id(("thread-tab", index))
                .flex()
                .items_center()
                .gap(px(6.0))
                .max_w(px(TAB_MAX_WIDTH))
                .pl(px(10.0))
                .pr(px(2.0))
                .py(px(3.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .cursor_pointer()
                .occlude()
                .text_size(px(12.0))
                .when(selected, |tab| tab.bg(color(theme::SURFACE)))
                .hover(|style| style.bg(color(theme::SURFACE)))
                .on_click(
                    cx.listener(move |app, _event, window, cx| app.open_thread(index, window, cx)),
                )
                .when(running, |tab| {
                    tab.child(div().size(px(7.0)).rounded_full().bg(color(theme::ACCENT)))
                })
                .child(div().overflow_hidden().whitespace_nowrap().child(title))
                .child(
                    Button::new(("close-thread", index))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .on_click(cx.listener(move |app, _event, window, cx| {
                            cx.stop_propagation();
                            app.close_thread(index, window, cx);
                        })),
                )
        }))
}

fn actions(app: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .occlude()
        .flex()
        .items_center()
        .gap(px(2.0))
        .pr(px(8.0))
        .child(
            Button::new("home")
                .ghost()
                .small()
                .icon(IconName::LayoutDashboard)
                .selected(app.page == Page::Home)
                .on_click(cx.listener(|app, _event, _window, cx| app.toggle_home(cx))),
        )
        .child(
            Button::new("toggle-rail")
                .ghost()
                .small()
                .icon(IconName::PanelRight)
                .selected(app.rail_open)
                .on_click(cx.listener(|app, _event, _window, cx| app.toggle_rail(cx))),
        )
        .child(
            Button::new("settings")
                .ghost()
                .small()
                .icon(IconName::Settings)
                .on_click(|_event, window: &mut Window, cx| settings::open(window, cx)),
        )
}
