//! Draws transcript items. Replies are markdown, rendered by Kit's `TextView`;
//! tool calls get their own views (see `toolcalls`).

use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div, px, relative};

use super::ChatView;
use super::items::Item;
use super::toolcalls;
use crate::theme::{self, color};

pub fn render_item(
    index: usize,
    item: &Item,
    folded: &HashSet<String>,
    cx: &mut Context<ChatView>,
) -> AnyElement {
    match item {
        Item::User(text) => user(text),
        Item::Assistant(text) => assistant(index, text),
        Item::Tool(tool) => toolcalls::render(tool, folded.contains(&tool.id), cx),
        Item::Approval { id, title, answer } => approval(id, title, *answer, cx),
        Item::Notice(text) => notice(text),
    }
}

/// A coding agent asking before it acts: Allow and Deny until answered, then what was chosen.
fn approval(id: &str, title: &str, answer: Option<bool>, cx: &mut Context<ChatView>) -> AnyElement {
    let card = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(12.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::WARN))
        .bg(color(theme::SURFACE))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().size(px(7.0)).flex_none().rounded_full().bg(color(theme::WARN)))
                .child(div().child(title.to_owned())),
        );
    match answer {
        Some(true) => card.child(div().text_color(color(theme::MUTED)).child("Allowed.")).into_any_element(),
        Some(false) => card.child(div().text_color(color(theme::MUTED)).child("Denied.")).into_any_element(),
        None => {
            let (allow_id, deny_id) = (id.to_owned(), id.to_owned());
            card.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(Button::new(SharedString::from(format!("allow-{id}"))).label("Allow").on_click(cx.listener(
                        move |chat, _event, _window, cx| chat.answer_approval(&allow_id, true, cx),
                    )))
                    .child(Button::new(SharedString::from(format!("deny-{id}"))).label("Deny").ghost().on_click(cx.listener(
                        move |chat, _event, _window, cx| chat.answer_approval(&deny_id, false, cx),
                    ))),
            )
            .into_any_element()
        }
    }
}

/// What you said: a tinted bubble on the right, so the two sides of the conversation are told apart at a glance.
fn user(text: &str) -> AnyElement {
    div()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(relative(0.82))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(theme::RADIUS_SURFACE))
                .border_1()
                .border_color(color(theme::ACCENT_LINE))
                .bg(color(theme::ACCENT_SOFT))
                .text_color(color(theme::TEXT))
                .child(text.to_owned()),
        )
        .into_any_element()
}

/// Something the app says that is not part of the conversation: a small chip with a dot, so it is never
/// mistaken for what the assistant wrote.
fn notice(text: &str) -> AnyElement {
    let problem = text.starts_with("Can't use") || text.starts_with("Signing in") && text.contains("failed");
    let dot = if problem { theme::WARN } else { theme::ACCENT };
    div()
        .flex()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(5.0))
                .rounded_full()
                .bg(color(theme::PANEL))
                .border_1()
                .border_color(color(theme::BORDER))
                .text_size(px(12.0))
                .text_color(color(theme::MUTED))
                .child(div().size(px(6.0)).flex_none().rounded_full().bg(color(dot)))
                .child(text.to_owned()),
        )
        .into_any_element()
}

fn assistant(index: usize, text: &str) -> AnyElement {
    TextView::markdown(
        SharedString::from(format!("reply-{index}")),
        text.to_owned(),
    )
    .selectable(true)
    .stream_fade(true)
    .into_any_element()
}
