//! Draws transcript items. Replies are markdown, rendered by Kit's `TextView`;
//! tool calls get their own views (see `toolcalls`).

use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div, px};

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
        Item::Notice(text) => div()
            .text_color(color(theme::MUTED))
            .child(text.clone())
            .into_any_element(),
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
        .border_color(color(theme::BORDER))
        .bg(color(theme::SURFACE))
        .child(div().child(title.to_owned()));
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

fn user(text: &str) -> AnyElement {
    div()
        .p(px(12.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::SURFACE))
        .child(text.to_owned())
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
