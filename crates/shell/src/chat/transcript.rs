//! Draws transcript items. Pure functions from data to elements.

use gpui::{AnyElement, SharedString, div, prelude::*, px};

use super::fences::{Segment, segments};
use super::items::{Item, ToolItem};
use crate::theme::{self, color};

const OUTPUT_MAX_HEIGHT: f32 = 240.0;

pub fn render_item(item: &Item) -> AnyElement {
    match item {
        Item::User(text) => user(text),
        Item::Assistant(text) => assistant(text),
        Item::Tool(tool) => tool_row(tool),
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

fn assistant(text: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP))
        .children(segments(text).into_iter().map(|segment| match segment {
            Segment::Prose(prose) => div().child(prose.to_owned()).into_any_element(),
            Segment::Code { lang, body } => code_block(lang, body),
        }))
        .into_any_element()
}

fn code_block(lang: &str, body: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(12.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::CODE_BG))
        .when(!lang.is_empty(), |block| {
            block.child(
                div()
                    .text_size(px(11.0))
                    .text_color(color(theme::MUTED))
                    .child(lang.to_owned()),
            )
        })
        .child(
            div()
                .font_family(theme::MONO)
                .text_size(px(13.0))
                .child(body.to_owned()),
        )
        .into_any_element()
}

fn tool_row(tool: &ToolItem) -> AnyElement {
    let dot = match (&tool.output, tool.is_error) {
        (None, _) => theme::ACCENT,
        (Some(_), true) => theme::DANGER,
        (Some(_), false) => theme::OK,
    };
    let header = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(13.0))
        .text_color(color(theme::MUTED))
        .child(div().size(px(8.0)).rounded_full().bg(color(dot)))
        .child(
            div()
                .text_color(color(theme::TEXT))
                .child(tool.name.clone()),
        )
        .child(div().font_family(theme::MONO).child(tool.summary.clone()));
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(header)
        .when_some(tool.output.as_ref(), |row, output| {
            row.child(output_box(&tool.id, output, tool.is_error))
        })
        .into_any_element()
}

/// Long output scrolls inside its own box instead of pushing the chat down.
fn output_box(tool_id: &str, output: &str, is_error: bool) -> impl IntoElement {
    let text = if is_error { theme::DANGER } else { theme::TEXT };
    div()
        .id(SharedString::from(format!("output-{tool_id}")))
        .max_h(px(OUTPUT_MAX_HEIGHT))
        .overflow_y_scroll()
        .p(px(10.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::CODE_BG))
        .text_color(color(text))
        .font_family(theme::MONO)
        .text_size(px(12.0))
        .child(output.to_owned())
}
