//! The chat screen: header, transcript, composer.

mod items;
mod session;
mod transcript;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, Subscription, Window, div, px,
};

use crate::theme::{self, color};
use items::{Item, apply};
use session::{Finished, Run, Session};

pub use session::Session as ChatSession;

const PLACEHOLDER: &str = "Message Vivianne…";
/// The composer grows with its text up to this many rows.
const COMPOSER_MAX_ROWS: usize = 8;
/// Content is measured one frame after it changes, so keep scrolling to the
/// bottom for a few frames after every update.
const PIN_FRAMES: u8 = 10;

pub struct ChatView {
    items: Vec<Item>,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    /// `Err` holds what is wrong with the setup, shown as the composer hint.
    session: Result<Session, String>,
    model: String,
    running: bool,
    /// Frames left in which to keep the transcript scrolled to the bottom.
    pin_frames: u8,
    _enter_to_send: Subscription,
}

impl ChatView {
    pub fn new(
        session: Result<Session, String>,
        model: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hint = match &session {
            Ok(_) => PLACEHOLDER.to_owned(),
            Err(problem) => problem.clone(),
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, COMPOSER_MAX_ROWS)
                .submit_on_enter(true)
                .placeholder(hint)
        });
        let enter_to_send = cx.subscribe_in(&input, window, |chat, _input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                chat.send(window, cx);
            }
        });
        Self {
            items: Vec::new(),
            input,
            scroll: ScrollHandle::new(),
            session,
            model,
            running: false,
            pin_frames: 0,
            _enter_to_send: enter_to_send,
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Ok(session) = self.session.as_mut() else {
            return;
        };
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some(run) = session.send(text.clone()) else {
            return;
        };
        self.input.update(cx, |input, cx| input.clean(window, cx));
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
            .children(
                self.items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| transcript::render_item(index, item)),
            );
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

    fn composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let send = Button::new("send")
            .primary()
            .label("Send")
            .on_click(cx.listener(|chat, _event, window, cx| chat.send(window, cx)));
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
                    .child(div().flex_1().child(Textarea::new(&self.input)))
                    .child(send),
            )
    }
}

impl Focusable for ChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pin_frames > 0 {
            self.pin_frames -= 1;
            self.scroll.scroll_to_bottom();
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::BG))
            .text_color(color(theme::TEXT))
            .text_size(px(14.0))
            .child(self.header())
            .child(self.transcript())
            .child(self.composer(cx))
    }
}
