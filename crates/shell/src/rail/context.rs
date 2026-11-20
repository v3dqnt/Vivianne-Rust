//! The Overview's "Context" card: how full the model's window is and what is filling it.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, div, px, relative};
use vivianne_core::Mode;
use vivianne_runtime::context::Breakdown;

use super::{muted, section};
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
    let (value, unit) = match tokens {
        0..1_000 => return tokens.to_string(),
        1_000..1_000_000 => (f64::from(tokens) / 1_000.0, "k"),
        _ => (f64::from(tokens) / 1_000_000.0, "M"),
    };
    // 1.0M reads better as 1M; 12.4k keeps its decimal.
    let text = format!("{value:.1}");
    format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
}

/// The parts are told apart by shade, not hue: a bar of five colours reads as a chart legend to decode, and the
/// accent is kept for what is selected.
const COLORS: [u32; 5] = [0x00dd_e1e8, 0x00a9_b0bc, 0x0078_808d, 0x0057_5e6a, 0x003d_434d];

pub fn card(view: &ContextView) -> AnyElement {
    let used = view.breakdown.total();
    let full = share(used, view.window);
        let rows = [
        view.breakdown.system,
        view.breakdown.tools,
        view.breakdown.user,
        view.breakdown.assistant,
        view.breakdown.tool_results,
    ];
    let labels = ["System prompt", "Tool definitions", "You", "Assistant", "Tool results"];
    let share_label = view.window.map(|_| if full < 0.1 { format!("{:.1}%", full * 100.0) } else { format!("{:.0}%", full * 100.0) });
    let mut lines = vec![muted(match view.window {
        Some(window) => format!("{} of {}", compact(used), compact(window)),
        None => format!("{} tokens", compact(used)),
    })
    .into_any_element()];
    // The bar only means something against a known window.
    if view.window.is_some() {
        let bar = div().flex().w_full().h(px(4.0)).rounded_full().overflow_hidden().bg(color(theme::RAISED));
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
                .child(div().size(px(8.0)).flex_none().rounded(px(2.0)).bg(color(hue)))
                .child(muted(*label).flex_1())
                .child(div().font_family(theme::MONO).text_size(px(theme::TEXT_SM)).child(compact(tokens)))
                .into_any_element(),
        );
    }
    super::section_with(
        "Context",
        share_label.map(|share| div().text_color(color(if full >= WARN_AT { theme::WARN } else { theme::TEXT })).child(share).into_any_element()),
        lines,
    )
    .into_any_element()
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
        assert_eq!(compact(1_000_000), "1M");
        assert_eq!(compact(128_000), "128k");
    }
}
