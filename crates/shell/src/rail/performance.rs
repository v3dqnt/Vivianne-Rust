//! Performance (Settings → Diagnostics): how smoothly the app is drawing, and what its background parts are doing.
//!
//! While this tab is open it asks for a new frame as soon as each one is drawn, so the numbers are the real pace of
//! the window: on a 120 Hz screen a smooth app shows about 120 fps and frames under 8.3 ms. Closing the tab ends the
//! constant redrawing, so it costs nothing when it is not open.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Window, div, px};

use crate::theme::{self, color};

/// Frames drawn as bars.
const BARS: usize = 120;
/// The slowest frame the graph has room for, in milliseconds.
const GRAPH_MAX_MS: f32 = 33.3;
const GRAPH_HEIGHT: f32 = 56.0;
/// A frame has this long at 120 Hz, at 144 Hz and at 60 Hz.
const BUDGET_120: f32 = 1000.0 / 120.0;
const BUDGET_144: f32 = 1000.0 / 144.0;
const BUDGET_60: f32 = 1000.0 / 60.0;
/// Lines of the log shown.
const LOG_LINES: usize = 80;

/// What the frames so far add up to.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub fps: f32,
    pub average_ms: f32,
    pub p99_ms: f32,
    pub worst_ms: f32,
    /// The share of frames, 0 to 1, that fit each budget.
    pub within_120: f32,
    pub within_144: f32,
    pub within_60: f32,
}

/// The numbers for a list of frame intervals in milliseconds. `None` with fewer than two frames.
#[must_use]
pub fn summarise(intervals: &[f32]) -> Option<Summary> {
    if intervals.len() < 2 {
        return None;
    }
    let count = intervals.len() as f32;
    let total: f32 = intervals.iter().sum();
    let average_ms = total / count;
    let mut sorted = intervals.to_vec();
    sorted.sort_by(f32::total_cmp);
    let p99_index = ((sorted.len() as f32 * 0.99).ceil() as usize).clamp(1, sorted.len()) - 1;
    let within =
        |budget: f32| intervals.iter().filter(|ms| **ms <= budget + 0.25).count() as f32 / count;
    Some(Summary {
        fps: if average_ms > 0.0 {
            1000.0 / average_ms
        } else {
            0.0
        },
        average_ms,
        p99_ms: sorted[p99_index],
        worst_ms: sorted[sorted.len() - 1],
        within_120: within(BUDGET_120),
        within_144: within(BUDGET_144),
        within_60: within(BUDGET_60),
    })
}

pub struct PerformanceView;

impl PerformanceView {
    /// Looks at the numbers twice a second while it is on screen (it is dropped with the tab's view).
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self
    }
}

fn bar_color(ms: f32) -> u32 {
    if ms <= BUDGET_120 + 0.25 {
        theme::OK
    } else if ms <= BUDGET_60 + 0.5 {
        theme::ACCENT
    } else {
        theme::DANGER
    }
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

fn time_of(at: u64) -> String {
    chrono::DateTime::from_timestamp(i64::try_from(at).unwrap_or(0), 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

impl Render for PerformanceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The numbers are the whole window's: the root view records every frame it draws. This tab only looks, and
        // looks again soon, so it keeps up while the app is being used.
        let _ = window;
        let continuous = crate::perf::continuous();
        let frames: Vec<f32> = crate::perf::intervals();
        let summary = summarise(&frames);
        let muted = |text: String| {
            div()
                .text_size(px(12.0))
                .text_color(color(theme::MUTED))
                .child(text)
        };

        let headline = summary
            .as_ref()
            .map_or_else(|| "measuring…".to_owned(), |s| format!("{:.0} fps", s.fps));
        let detail = summary.as_ref().map_or_else(String::new, |s| {
            format!(
                "average {:.1} ms · 99% under {:.1} ms · worst {:.1} ms",
                s.average_ms, s.p99_ms, s.worst_ms
            )
        });
        let budgets = summary.as_ref().map_or_else(String::new, |s| {
            format!(
                "on time for 60 Hz {} · 120 Hz {} · 144 Hz {}",
                percent(s.within_60),
                percent(s.within_120),
                percent(s.within_144)
            )
        });

        let recent = frames.len().saturating_sub(BARS);
        let bars = frames[recent..].iter().map(|ms| {
            let height = (ms.min(GRAPH_MAX_MS) / GRAPH_MAX_MS * GRAPH_HEIGHT).max(1.0);
            div()
                .w(px(2.0))
                .h(px(height))
                .flex_none()
                .bg(color(bar_color(*ms)))
        });
        // The line for the 120 Hz budget, drawn across the graph.
        let budget_line = (BUDGET_120 / GRAPH_MAX_MS * GRAPH_HEIGHT).round();

        let log = vivianne_core::debuglog::entries();
        let log_rows = log.iter().take(LOG_LINES).map(|e| {
            div()
                .flex()
                .gap(px(8.0))
                .font_family(theme::MONO)
                .text_size(px(11.0))
                .child(
                    div()
                        .flex_none()
                        .text_color(color(theme::MUTED))
                        .child(time_of(e.at)),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(44.0))
                        .text_color(color(theme::ACCENT))
                        .child(e.category),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_color(color(theme::TEXT))
                        .child(e.text.clone()),
                )
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .px(px(20.0))
            .py(px(14.0))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_size(px(26.0)).text_color(color(theme::TEXT)).child(headline))
                    .child(muted(detail))
                    .child(muted(budgets)),
            )
            .child(
                div()
                    .relative()
                    .h(px(GRAPH_HEIGHT))
                    .w_full()
                    .flex()
                    .items_end()
                    .gap(px(0.0))
                    .overflow_hidden()
                    .child(div().absolute().left_0().right_0().bottom(px(budget_line)).h(px(1.0)).bg(color(theme::BORDER)))
                    .children(bars),
            )
            .child(muted("Bars are frame times of the whole window as you use it; the line is the 120 Hz budget (8.3 ms). The app draws only when something changes, so idle shows nothing.".to_owned()))
            .child(
                div().flex().gap(px(8.0)).child(
                    Button::new("debug-continuous")
                        .outline()
                        .label(if continuous { "Stop redrawing" } else { "Redraw continuously" })
                        .on_click(cx.listener(move |_view, _e, window, cx| {
                            crate::perf::set_continuous(!crate::perf::continuous());
                            window.refresh();
                            cx.notify();
                        })),
                ).child(
                    Button::new("debug-reset").ghost().label("Reset").on_click(cx.listener(|_view, _e, _w, cx| {
                        crate::perf::reset();
                        cx.notify();
                    })),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_color(color(theme::TEXT)).child("Log"))
                    .child(
                        Button::new("debug-clear")
                            .ghost()
                            .label("Clear")
                            .on_click(cx.listener(|_view, _e, _w, cx| {
                                vivianne_core::debuglog::clear();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("debug-log")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .overflow_y_scroll()
                    .children(log_rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steady_120_hz_run_reads_as_120_fps_and_all_frames_on_time() {
        let frames = vec![8.33_f32; 100];
        let s = summarise(&frames).expect("enough frames");
        assert!((s.fps - 120.0).abs() < 1.0);
        assert_eq!((s.within_120, s.within_60), (1.0, 1.0));
        assert!(s.within_144 < 0.01, "8.3 ms is too slow for 144 Hz");
    }

    #[test]
    fn one_slow_frame_shows_in_the_worst_and_the_tail_not_the_average() {
        let mut frames = vec![8.33_f32; 99];
        frames.push(50.0);
        let s = summarise(&frames).expect("enough frames");
        assert_eq!(s.worst_ms, 50.0);
        assert!(s.average_ms < 9.0 && s.p99_ms <= 50.0);
        assert!((s.within_120 - 0.99).abs() < 0.001);
        assert!(summarise(&[8.0]).is_none());
    }

    #[test]
    fn bars_are_green_within_the_120_hz_budget_amber_to_60_and_red_beyond() {
        assert_eq!(bar_color(8.0), theme::OK);
        assert_eq!(bar_color(15.0), theme::ACCENT);
        assert_eq!(bar_color(40.0), theme::DANGER);
    }
}
