//! The title bar: thread tabs in the middle, Home / rail / Settings on the right.

use gpui_kit::base::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{IconName, Sizable as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{Context, FontWeight, Hsla, SharedString, Window, div, px, rgb};

use super::{AppView, Page};
use crate::settings;
use crate::theme::{self, color};

const TAB_MAX_WIDTH: f32 = 220.0;

pub fn render(app: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // No rule under the bar: it and the page below it are one surface.
    TitleBar::new().border_b_0().bg(theme::glass()).child(
        div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .pl(px(4.0))
                    .pr(px(10.0))
                    .text_size(px(theme::TEXT_SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(theme::FAINT))
                    .child("Vivianne"),
            )
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
            let group = SharedString::from(format!("thread-tab-{index}"));
            div()
                .id(("thread-tab", index))
                .group(group.clone())
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(28.0))
                .max_w(px(TAB_MAX_WIDTH))
                .pl(px(12.0))
                .pr(px(4.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .cursor_pointer()
                .occlude()
                .text_size(px(theme::TEXT_SM))
                .text_color(color(if selected { theme::TEXT } else { theme::MUTED }))
                .when(selected, |tab| tab.bg(color(theme::RAISED)))
                .when(!selected, |tab| {
                    tab.hover(|style| {
                        style
                            .bg(color(theme::SURFACE))
                            .text_color(color(theme::TEXT))
                    })
                })
                .on_click(
                    cx.listener(move |app, _event, window, cx| app.open_thread(index, window, cx)),
                )
                .child(div().overflow_hidden().whitespace_nowrap().child(title))
                // Working: a spinner where the close button goes. Otherwise the close button, shown on the open tab
                // and under the pointer, so a row of tabs is not a row of crosses.
                .child(if running {
                    div()
                        .size(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Spinner::new()
                                .xsmall()
                                .color(Hsla::from(rgb(theme::ACCENT))),
                        )
                        .into_any_element()
                } else {
                    div()
                        .when(!selected, |slot| {
                            slot.invisible().group_hover(group, |style| style.visible())
                        })
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
                        .into_any_element()
                })
        }))
}

fn actions(app: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .occlude()
        .flex()
        .items_center()
        .gap(px(2.0))
        .pr(px(8.0))
        .when(matches!(app.page, Page::Thread(_)), |bar| {
            let view = cx.entity();
            bar.child(
                Button::new("export-thread")
                    .ghost()
                    .small()
                    .icon(IconName::ExternalLink)
                    .tooltip("Export this conversation")
                    .dropdown_menu(move |menu, _window, _cx| {
                        let item = |label: &'static str, zip: bool| {
                            let view = view.clone();
                            PopupMenuItem::new(label).on_click(move |_event, window, cx| {
                                view.update(cx, |app, cx| app.export_thread(zip, window, cx));
                            })
                        };
                        menu.item(item("Export as Markdown…", false))
                            .item(item("Export with attachments (ZIP)…", true))
                    }),
            )
        })
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
