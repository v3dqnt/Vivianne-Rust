//! The chat screen: header, transcript, composer.

mod composer;
mod fences;
mod items;
mod session;
mod transcript;

use gpui::{
    Context, FocusHandle, Focusable, KeyDownEvent, ScrollHandle, Window, div, prelude::*, px,
};

use crate::theme::{self, color};
use composer::Composer;
use items::{Item, apply};
use session::{Finished, Run, Session};

pub use session::Session as ChatSession;

pub struct ChatView {
    items: Vec<Item>,
    composer: Composer,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// `Err` holds what is wrong with the setup, shown in place of the placeholder.
    session: Result<Session, String>,
    model: String,
    running: bool,
    /// Frames left in which to keep the transcript scrolled to the bottom.
    pin_frames: u8,
}

const PIN_FRAMES: u8 = 10;

impl ChatView {
    pub fn new(session: Result<Session, String>, model: String, cx: &mut Context<Self>) -> Self {
        Self {
            items: Vec::new(),
            composer: Composer::default(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            session,
            model,
            running: false,
            pin_frames: 0,
        }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Ok(session) = self.session.as_mut() else {
            return;
        };
        let Some(text) = self.composer.take() else {
            return;
        };
        let Some(run) = session.send(text.clone()) else {
            return;
        };
        self.items.push(Item::User(text));
        self.running = true;
        self.pin_frames = PIN_FRAMES;
        let Run { mut events, done } = run;
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                let alive = this.update(cx, |chat, cx| {
                    apply(&mut chat.items, &event);
                    chat.pin_frames = PIN_FRAMES;
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            if let Ok(finished) = done.await {
                this.update(cx, |chat, cx| chat.finish(finished, cx)).ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn finish(&mut self, finished: Finished, cx: &mut Context<Self>) {
        self.running = false;
        self.pin_frames = PIN_FRAMES;
        if let Ok(session) = self.session.as_mut()
            && let Some(error) = session.finish(finished)
        {
            self.items.push(Item::Notice(format!("Error: {error}")));
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let mods = key.modifiers;
        match key.key.as_str() {
            "enter" if mods.shift => self.composer.insert("\n"),
            "enter" => return self.send(cx),
            "space" => self.composer.insert(" "),
            "backspace" => self.composer.backspace(),
            "v" if mods.control || mods.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|clip| clip.text()) {
                    self.composer.insert(&text);
                }
            }
            _ => {
                // AltGr arrives as ctrl+alt, so plain ctrl alone is the only blocker.
                let command = mods.platform || (mods.control && !mods.alt);
                if let (Some(typed), false) = (&key.key_char, command) {
                    self.composer.insert(typed);
                }
            }
        }
        cx.notify();
    }

    fn header(&self) -> impl IntoElement {
        let status = if self.running { "Working" } else { "Ready" };
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .px(px(theme::SIDE_PAD))
            .py(px(10.0))
            .border_b_1()
            .border_color(color(theme::BORDER))
            .child(div().text_size(px(15.0)).child("Vivianne"))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(color(theme::MUTED))
                    .child(format!("{}  ·  {status}", self.model)),
            )
    }

    fn transcript(&self) -> impl IntoElement {
        let column = div()
            .w_full()
            .max_w(px(theme::COLUMN_MAX))
            .flex()
            .flex_col()
            .gap(px(theme::GAP))
            .children(self.items.iter().map(transcript::render_item));
        div()
            .id("transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(theme::SIDE_PAD))
            .py(px(16.0))
            .flex()
            .flex_col()
            .items_center()
            .child(column)
    }

    fn composer_box(&self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (shown, shown_color) = match (self.composer.text(), &self.session) {
            ("", Ok(_)) => ("Message Vivianne…".to_owned(), theme::MUTED),
            ("", Err(problem)) => (problem.clone(), theme::DANGER),
            (text, _) => (
                format!("{text}{}", if focused { "▏" } else { "" }),
                theme::TEXT,
            ),
        };
        let input = div()
            .id("composer")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|chat, event, _window, cx| chat.on_key(event, cx)))
            .flex_1()
            .max_h(px(220.0))
            .overflow_hidden()
            .text_color(color(shown_color))
            .child(shown);
        let send = div()
            .id("send")
            .cursor_pointer()
            .px(px(12.0))
            .py(px(6.0))
            .rounded(px(theme::RADIUS_CONTROL))
            .bg(color(theme::ACCENT))
            .on_click(cx.listener(|chat, _event, _window, cx| chat.send(cx)))
            .child("Send");
        div()
            .px(px(theme::SIDE_PAD))
            .pb(px(16.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(theme::COLUMN_MAX))
                    .flex()
                    .items_end()
                    .gap(px(10.0))
                    .p(px(12.0))
                    .rounded(px(theme::RADIUS_SURFACE))
                    .border_1()
                    .border_color(color(theme::BORDER))
                    .bg(color(theme::SURFACE))
                    .child(input)
                    .child(send),
            )
    }
}

impl Focusable for ChatView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Content is measured one frame after it changes, so keep scrolling to the
        // bottom for a few frames after every update.
        if self.pin_frames > 0 {
            self.pin_frames -= 1;
            self.scroll.scroll_to_bottom();
            window.request_animation_frame();
        }
        let focused = self.focus.is_focused(window);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::BG))
            .text_color(color(theme::TEXT))
            .text_size(px(14.0))
            .child(self.header())
            .child(self.transcript())
            .child(self.composer_box(focused, cx))
    }
}
