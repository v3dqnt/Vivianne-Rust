//! The Terminal tab: a real shell in the project folder.

mod keys;
mod screen;
mod session;

use std::ops::Range;
use std::path::Path;

use futures::StreamExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, FocusHandle, FontWeight, HighlightStyle, Hsla, KeyDownEvent, MouseButton, ScrollEvent,
    StyledText, UnderlineStyle, Window, div, px, rgb,
};

use self::keys::{Press, bytes_for};
use self::screen::{Rgb, Span, fold_line};
use self::session::{Screen, Session, Signal};
use crate::theme::{self, color};

/// Drawn size of one character and one line, at the 12 px terminal font.
const CELL_WIDTH: f32 = 6.9;
const LINE_HEIGHT: f32 = 16.0;
/// Space around the terminal inside the rail: padding, tab bar, borders.
const SIDE_CHROME: f32 = 28.0;
const TOP_CHROME: f32 = 130.0;
const START_COLUMNS: u16 = 48;
const START_LINES: u16 = 24;

pub struct TerminalView {
    session: Result<Session, String>,
    ended: bool,
    focus: FocusHandle,
    /// A size the view keeps, for a terminal that is not the rail's (it does not follow the window).
    fixed: bool,
}

impl TerminalView {
    pub fn new(root: &Path, cx: &mut Context<Self>) -> Self {
        Self::open(root, None, (START_COLUMNS, START_LINES), false, cx)
    }

    /// A terminal of a fixed size that runs `command` first, for a window of its own.
    pub fn running(
        root: &Path,
        command: &str,
        (columns, lines): (u16, u16),
        cx: &mut Context<Self>,
    ) -> Self {
        Self::open(root, Some(command), (columns, lines), true, cx)
    }

    fn open(
        root: &Path,
        command: Option<&str>,
        (columns, lines): (u16, u16),
        fixed: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = match Session::start(root, columns, lines, command) {
            Ok((session, signals)) => {
                Self::listen(signals, cx);
                Ok(session)
            }
            Err(why) => Err(why),
        };
        Self {
            session,
            ended: false,
            focus: cx.focus_handle(),
            fixed,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Redraws when the shell prints, answers its queries, and notes when it ends.
    fn listen(
        mut signals: futures::channel::mpsc::UnboundedReceiver<Signal>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some(first) = signals.next().await {
                let mut batch = vec![first];
                while let Ok(more) = signals.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |terminal, cx| terminal.apply(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply(&mut self, signals: Vec<Signal>, cx: &mut Context<Self>) {
        for signal in signals {
            match signal {
                Signal::Redraw => {}
                Signal::Reply(text) => {
                    if let Ok(session) = &self.session {
                        session.write(text.into_bytes());
                    }
                }
                Signal::Exited => self.ended = true,
            }
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Ok(session) = &self.session else { return };
        let stroke = &event.keystroke;
        let press = Press {
            key: &stroke.key,
            text: stroke.key_char.as_deref(),
            ctrl: stroke.modifiers.control,
            alt: stroke.modifiers.alt,
            shift: stroke.modifiers.shift,
        };
        if let Some(bytes) = bytes_for(&press) {
            session.scroll_to_bottom();
            session.write(bytes);
            cx.stop_propagation();
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Ok(session) = &self.session else { return };
        let moved = f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y) / LINE_HEIGHT;
        #[allow(clippy::cast_possible_truncation)]
        let lines = moved.round() as i32;
        if lines != 0 {
            session.scroll(lines);
            cx.notify();
        }
    }

    /// Keeps the shell's idea of its size in step with the window.
    fn fit(&mut self, window: &Window) {
        if self.fixed {
            return;
        }
        let Ok(session) = &mut self.session else {
            return;
        };
        let height = f32::from(window.viewport_size().height);
        let columns = (crate::rail::WIDTH - SIDE_CHROME) / CELL_WIDTH;
        let lines = (height - TOP_CHROME) / LINE_HEIGHT;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        session.resize(columns.max(2.0) as u16, lines.max(1.0) as u16);
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit(window);
        let body = match &self.session {
            Ok(session) => screen_lines(&session.screen()).into_any_element(),
            Err(why) => div()
                .text_color(color(theme::DANGER))
                .child(why.clone())
                .into_any_element(),
        };
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|terminal, event: &KeyDownEvent, _window, cx| {
                terminal.key_down(event, cx)
            }))
            .on_scroll(
                cx.listener(|terminal, event: &ScrollEvent, _window, cx| {
                    terminal.scroll(event, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|terminal, _event, window, cx| window.focus(&terminal.focus, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .p(px(6.0))
            .rounded(px(theme::RADIUS_CONTROL))
            .bg(color(theme::CODE_BG))
            .font_family(theme::MONO)
            .text_size(px(12.0))
            .line_height(px(LINE_HEIGHT))
            .child(body)
            .children(self.ended.then(|| {
                div()
                    .pt(px(6.0))
                    .text_color(color(theme::MUTED))
                    .child("The shell has ended. Close and reopen the tab to start another.")
            }))
    }
}

fn screen_lines(screen: &Screen) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .children(screen.lines.iter().map(|cells| {
            let (text, spans) = fold_line(cells);
            let text = if text.is_empty() {
                " ".to_owned()
            } else {
                text
            };
            let highlights: Vec<(Range<usize>, HighlightStyle)> = spans
                .iter()
                .map(|span| (span.range.clone(), style_of(span)))
                .collect();
            div()
                .h(px(LINE_HEIGHT))
                .whitespace_nowrap()
                .child(StyledText::new(text).with_highlights(highlights))
        }))
}

fn style_of(span: &Span) -> HighlightStyle {
    HighlightStyle {
        color: Some(hsla(span.fg)),
        background_color: span.bg.map(hsla),
        font_weight: span.bold.then_some(FontWeight::BOLD),
        underline: span.underline.then(|| UnderlineStyle {
            thickness: px(1.0),
            color: None,
            wavy: false,
        }),
        ..HighlightStyle::default()
    }
}

fn hsla((r, g, b): Rgb) -> Hsla {
    rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
}
