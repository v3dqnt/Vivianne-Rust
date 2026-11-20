//! The Overview's "Context" card: how full the model's window is and what is filling it.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, div, px, relative};
use vivianne_core::Mode;
use vivianne_runtime::context::Breakdown;

use super::{muted, pill, section};
use crate::theme::{self, color};

/// Above this share of the window the figure turns amber: the model is about to start losing the oldest turns.
const WARN_AT: f32 = 0.8;

/// What the card shows: the parts, the size of the window (when the provider says), and the mode in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextView {
    pub breakdown: Breakdown,
    pub window: Option<u32>,
    pub mode: Mode,
}

/// `part` as a share of `whole`, 0 to 1. An unknown or empty window is 0, not a division by zero.
fn share(part: u32, whole: Option<u32>) -> f32 {
    match whole {
        Some(whole) if whole > 0 => (part as f32 / whole as f32).min(1.0),
        _ => 0.0,
    }
}

/// `12400` as `12.4k`.
fn compact(tokens: u32) -> String {
    if tokens < 1000 { tokens.to_string() } else { format!("{:.1}k", tokens as f32 / 1000.0) }
}

const COLORS: [u32; 5] = [theme::ACCENT, theme::WARN, theme::ACCENT_TEXT, theme::OK, theme::MUTED];

pub fn card(view: &ContextView) -> AnyElement {
    let used = view.breakdown.total();
    let full = share(used, view.window);
    let headline = match view.window {
        Some(window) => format!("{} of {} tokens", compact(used), compact(window)),
        None => format!("{} tokens", compact(used)),
    };
    let rows = [
        view.breakdown.system,
        view.breakdown.tools,
        view.breakdown.user,
        view.breakdown.assistant,
        view.breakdown.tool_results,
    ];
    let labels = ["System prompt", "Tool definitions", "You", "Assistant", "Tool results"];
    let mut lines = vec![
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(div().text_color(color(if full >= WARN_AT { theme::WARN } else { theme::TEXT })).child(headline))
            .child(pill(view.mode.label().to_owned()))
            .into_any_element(),
    ];
    // The bar only means something against a known window.
    if view.window.is_some() {
        let bar = div().flex().w_full().h(px(6.0)).rounded_full().overflow_hidden().bg(color(theme::CODE_BG));
        let bar = rows.iter().zip(COLORS).fold(bar, |bar, (&tokens, hue)| bar.child(div().h_full().w(relative(share(tokens, view.window))).bg(color(hue))));
        lines.push(bar.into_any_element());
    }
    for ((label, tokens), hue) in labels.iter().zip(rows).zip(COLORS) {
        if tokens == 0 {
            continue;
        }
        lines.push(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().size(px(7.0)).flex_none().rounded_full().bg(color(hue)))
                .child(muted(*label).flex_1())
                .child(muted(compact(tokens)))
                .into_any_element(),
        );
    }
    section("Context", lines).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_is_capped_and_an_unknown_window_is_empty() {
        assert!((share(50, Some(200)) - 0.25).abs() < f32::EPSILON);
        assert!((share(300, Some(200)) - 1.0).abs() < f32::EPSILON);
        assert!(share(5, None) == 0.0 && share(5, Some(0)) == 0.0);
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_400), "12.4k");
    }
}
