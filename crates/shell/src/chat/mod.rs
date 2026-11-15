//! The chat screen: header, transcript, composer.

mod items;
mod session;
mod transcript;

use gpui_kit::component::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, ScrollHandle, Subscription, Window,
    div, px,
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
    /// Name of the project folder, shown on the empty screen.
    project: String,
    running: bool,
    /// Frames left in which to keep the transcript scrolled to the bottom.
    pin_frames: u8,
    _enter_to_send: Subscription,
}

/// Starting points shown before the first message. Picking one fills the composer.
const SUGGESTIONS: [&str; 3] = [
    "Summarise this project",
    "Find and fix a bug",
    "Write tests for a module",
];

impl ChatView {
    pub fn new(
        session: Result<Session, String>,
        model: String,
        project: String,
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
            project,
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

    /// Tab title: the first thing the user said, shortened.
    #[must_use]
    pub fn title(&self) -> String {
        let first = self.items.iter().find_map(|item| match item {
            Item::User(text) => Some(text.as_str()),
            _ => None,
        });
        match first {
            None => NEW_THREAD_TITLE.to_owned(),
            Some(text) => shorten(text, TITLE_MAX_CHARS),
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Small status line under the composer.
    fn status_line(&self) -> impl IntoElement {
        let status = if self.running { "Working" } else { "Ready" };
        div()
            .pb(px(10.0))
            .flex()
            .justify_center()
            .text_size(px(11.0))
            .text_color(color(theme::MUTED))
            .child(format!("{}  ·  {status}", self.model))
    }

    /// The screen before the first message: a question and a few starting points.
    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(theme::SIDE_PAD))
            .child(
                div()
                    .w_full()
                    .max_w(px(theme::COLUMN_MAX))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(26.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child("What are we working on?"),
                    )
                    .child(div().text_color(color(theme::MUTED)).child(format!(
                        "Working in {}. Ask a question, or have the agent change something.",
                        self.project
                    )))
                    .child(div().h(px(14.0)))
                    .children(SUGGESTIONS.iter().enumerate().map(|(index, text)| {
                        div()
                            .id(("suggestion", index))
                            .px(px(14.0))
                            .py(px(10.0))
                            .rounded(px(theme::RADIUS_SURFACE))
                            .border_1()
                            .border_color(color(theme::BORDER))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .hover(|row| row.bg(color(theme::SURFACE)))
                            .on_click(cx.listener(move |chat, _event, window, cx| {
                                chat.prefill(SUGGESTIONS[index], window, cx);
                            }))
                            .child(*text)
                    })),
            )
    }

    fn prefill(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        window.focus(&self.input.read(cx).focus_handle(cx), cx);
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
            .icon(IconName::ArrowUp)
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
            .child(if self.items.is_empty() {
                self.empty_state(cx).into_any_element()
            } else {
                self.transcript().into_any_element()
            })
            .child(self.composer(cx))
            .child(self.status_line())
    }
}

const NEW_THREAD_TITLE: &str = "New thread";
const TITLE_MAX_CHARS: usize = 28;

/// Cut `text` to `max` characters on one line, adding an ellipsis when shortened.
fn shorten(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let cut: String = one_line.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn short_text_is_kept_whole() {
        assert_eq!(shorten("fix the bug", 28), "fix the bug");
    }

    #[test]
    fn newlines_collapse_to_one_line() {
        assert_eq!(shorten("fix\n  the\nbug", 28), "fix the bug");
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis_on_characters_not_bytes() {
        assert_eq!(shorten("ééééé ééééé", 7), "ééééé é…");
    }
}
