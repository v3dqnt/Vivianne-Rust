//! Draws transcript items. Replies are markdown, rendered by Kit's `TextView`;
//! tool calls get their own views (see `toolcalls`).

use std::collections::HashSet;

use gpui_kit::component::Sizable as _;
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
        // A tool call shows one line until it is opened; a failure shows its output, since that is what to read.
        Item::Tool(tool) => toolcalls::render(tool, !folded.contains(&tool.id) && !tool.is_error, cx),
        Item::Approval { id, title, answer } => approval(id, title, *answer, cx),
        Item::Notice(text) => notice(text),
    }
}

/// A coding agent asking before it acts: Allow and Deny until answered, then what was chosen.
fn approval(id: &str, title: &str, answer: Option<bool>, cx: &mut Context<ChatView>) -> AnyElement {
    let card = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .pl(px(14.0))
        .pr(px(12.0))
        .py(px(12.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::BORDER))
        .border_l_2()
        .bg(color(theme::SURFACE))
        .child(div().text_size(px(theme::TEXT_XS)).text_color(color(theme::WARN)).child("Needs your approval"))
        .child(div().font_family(theme::MONO).text_size(px(theme::TEXT_SM)).child(title.to_owned()));
    let card = card.border_color(color(if answer.is_none() { theme::WARN } else { theme::BORDER }));
    match answer {
        Some(true) => card.child(div().text_color(color(theme::MUTED)).child("Allowed.")).into_any_element(),
        Some(false) => card.child(div().text_color(color(theme::MUTED)).child("Denied.")).into_any_element(),
        None => {
            let (allow_id, deny_id) = (id.to_owned(), id.to_owned());
            card.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(Button::new(SharedString::from(format!("allow-{id}"))).label("Allow").primary().small().on_click(cx.listener(
                        move |chat, _event, _window, cx| chat.answer_approval(&allow_id, true, cx),
                    )))
                    .child(Button::new(SharedString::from(format!("deny-{id}"))).label("Deny").ghost().small().on_click(cx.listener(
                        move |chat, _event, _window, cx| chat.answer_approval(&deny_id, false, cx),
                    ))),
            )
            .into_any_element()
        }
    }
}

/// What you said: a quiet bubble on the right, so the two sides of the conversation are told apart at a glance
/// without the accent colour on every message.
fn user(text: &str) -> AnyElement {
    div()
        .flex()
        .justify_end()
        .pt(px(6.0))
        .child(
            div()
                .max_w(relative(0.8))
                .px(px(14.0))
                .py(px(9.0))
                .rounded(px(theme::RADIUS_COMPOSER))
                .bg(color(theme::RAISED))
                .text_color(color(theme::TEXT))
                .child(text.to_owned()),
        )
        .into_any_element()
}

/// Something the app says that is not part of the conversation: a small centred line between hairlines, so it
/// reads as a marker in the thread and is never mistaken for what the assistant wrote. A problem is in amber.
fn notice(text: &str) -> AnyElement {
    let problem = text.starts_with("Can't use") || text.starts_with("Error") || text.starts_with("Couldn't") || (text.starts_with("Signing in") && text.contains("failed"));
    let rule = || div().flex_1().h(px(1.0)).bg(color(theme::HAIRLINE));
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(rule())
        .child(
            div()
                .max_w(relative(0.8))
                .text_size(px(theme::TEXT_SM))
                .text_color(color(if problem { theme::WARN } else { theme::MUTED }))
                .child(text.to_owned()),
        )
        .child(rule())
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
