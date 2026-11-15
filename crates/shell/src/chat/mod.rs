//! The chat screen: transcript (or the empty state), composer, status line.

mod composer;
mod items;
mod session;
mod transcript;

use std::path::PathBuf;

use gpui_kit::component::input::InputEvent;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, FontWeight, PathPromptOptions, ScrollHandle,
    Subscription, Window, div, px,
};

use crate::theme::{self, color};
use composer::{Composer, Thinking, compose_message, display_message};
use items::{Item, apply};
use session::{Finished, Run, Session};

pub use session::Session as ChatSession;

const PLACEHOLDER: &str = "Message Vivianne…";
/// Content is measured one frame after it changes, so keep scrolling to the
/// bottom for a few frames after every update.
const PIN_FRAMES: u8 = 10;

pub struct ChatView {
    items: Vec<Item>,
    composer: Composer,
    scroll: ScrollHandle,
    /// `Err` holds what is wrong with the setup, shown as the composer hint.
    session: Result<Session, String>,
    model: String,
    /// Name of the project folder, shown on the empty screen.
    project: String,
    running: bool,
    /// Frames left in which to keep the transcript scrolled to the bottom.
    pin_frames: u8,
    _input_events: Subscription,
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
        let composer = Composer::new(hint, window, cx);
        let input_events = cx.subscribe_in(
            &composer.input,
            window,
            |chat, _input, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => chat.send(window, cx),
                    // The send button's enabled state follows the text.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            },
        );
        Self {
            items: Vec::new(),
            composer,
            scroll: ScrollHandle::new(),
            session,
            model,
            project,
            running: false,
            pin_frames: 0,
            _input_events: input_events,
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Ok(session) = self.session.as_mut() else {
            return;
        };
        let text = self.composer.input.read(cx).value().to_string();
        let files = &self.composer.attachments;
        if text.trim().is_empty() && files.is_empty() {
            return;
        }
        let Some(run) = session.send(compose_message(&text, files)) else {
            return;
        };
        self.items.push(Item::User(display_message(&text, files)));
        self.composer.attachments.clear();
        self.composer
            .input
            .update(cx, |input, cx| input.clean(window, cx));
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

    /// Stop the run in progress.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.running {
            return;
        }
        if let Ok(session) = self.session.as_mut() {
            session.cancel();
        }
        self.running = false;
        self.pin_frames = PIN_FRAMES;
        self.items.push(Item::Notice("Stopped.".to_owned()));
        cx.notify();
    }

    /// Open the system file picker and attach what is chosen.
    fn pick_files(cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                this.update(cx, |chat, cx| chat.attach_paths(&paths, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn attach_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.composer.attach(paths);
        cx.notify();
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.composer.attachments.len() {
            self.composer.attachments.remove(index);
            cx.notify();
        }
    }

    fn set_thinking(&mut self, level: Thinking, cx: &mut Context<Self>) {
        self.composer.thinking = level;
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
        self.composer
            .input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        window.focus(&self.composer.input.read(cx).focus_handle(cx), cx);
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
}

impl Focusable for ChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.input.read(cx).focus_handle(cx)
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
            .child(composer::render(self, window, cx))
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
