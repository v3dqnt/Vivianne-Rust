//! Phase 1 spike (a): a long transcript that streams text while we measure
//! how often and how slowly frames render. Prints a report and exits after
//! `SECONDS`. Throwaway; findings go in docs/spikes.md.

#![allow(clippy::pedantic, clippy::expect_used)] // throwaway spike

use std::time::{Duration, Instant};

use gpui::{
    App, Application, Bounds, Context, ScrollHandle, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};

const SECONDS: u64 = 8;
/// Roughly a fast model: ~1500 chars/second in 16 ms ticks.
const CHARS_PER_TICK: usize = 24;
const LOREM: &str = "The agent reads the project, plans the change and edits files, \
then runs the tests and reports what passed. ";
const CODE: &str =
    "fn main() {\n    let total: u32 = (1..=10).sum();\n    println!(\"{total}\");\n}";

enum Block {
    Text(String),
    Code(String),
}

struct Transcript {
    blocks: Vec<Block>,
    scroll: ScrollHandle,
    started: Instant,
    last_render: Option<Instant>,
    frame_ms: Vec<f64>,
    ticks: usize,
}

impl Transcript {
    fn new() -> Self {
        // Start with a long history so layout cost is realistic.
        let mut blocks = Vec::new();
        for i in 0..200 {
            blocks.push(Block::Text(format!("{i}: {}", LOREM.repeat(3))));
            if i % 10 == 0 {
                blocks.push(Block::Code(CODE.to_owned()));
            }
        }
        blocks.push(Block::Text(String::new()));
        Self {
            blocks,
            scroll: ScrollHandle::new(),
            started: Instant::now(),
            last_render: None,
            frame_ms: Vec::new(),
            ticks: 0,
        }
    }

    fn stream_tick(&mut self, tick: usize) {
        if let Some(Block::Text(text)) = self.blocks.last_mut() {
            text.extend(
                LOREM
                    .chars()
                    .cycle()
                    .skip(tick * CHARS_PER_TICK)
                    .take(CHARS_PER_TICK),
            );
        }
    }

    fn report(&self) -> String {
        let mut sorted = self.frame_ms.clone();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len().max(1);
        let pick = |q: f64| {
            sorted
                .get(((n as f64) * q) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let avg = sorted.iter().sum::<f64>() / n as f64;
        format!(
            "frames={} avg_gap={avg:.1}ms p95_gap={:.1}ms max_gap={:.1}ms fps~{:.0}",
            sorted.len(),
            pick(0.95),
            sorted.last().copied().unwrap_or(0.0),
            1000.0 / avg.max(0.001),
        )
    }
}

impl Render for Transcript {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drive streaming from the frame loop itself: one tick per frame, so the
        // gap between renders is the real frame pacing, not a 15.6 ms Windows timer.
        window.request_animation_frame();
        let now = Instant::now();
        if let Some(prev) = self.last_render.replace(now) {
            self.frame_ms
                .push(now.duration_since(prev).as_secs_f64() * 1000.0);
            self.ticks += 1;
            self.stream_tick(self.ticks);
            self.scroll.scroll_to_bottom();
        }
        if self.started.elapsed() > Duration::from_secs(SECONDS) {
            println!("{}", self.report());
            cx.quit();
        }
        div()
            .id("transcript")
            .size_full()
            .bg(rgb(0x14161a))
            .text_color(rgb(0xe6e6e6))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .children(self.blocks.iter().map(|block| {
                match block {
                    Block::Text(text) => div().child(text.clone()),
                    Block::Code(code) => div()
                        .p(px(12.0))
                        .rounded(px(10.0))
                        .bg(rgb(0x1e2128))
                        .font_family("Consolas")
                        .child(code.clone()),
                }
            }))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.0), px(700.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|_| Transcript::new()))
            .expect("open window");
        cx.activate(true);
    });
}
