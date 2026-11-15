//! Draws transcript items. Replies are markdown, rendered by Kit's `TextView`;
//! tool calls get their own views (see `toolcalls`).

use std::collections::HashSet;

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
        Item::Notice(text) => div()
            .text_color(color(theme::MUTED))
            .child(text.clone())
            .into_any_element(),
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
