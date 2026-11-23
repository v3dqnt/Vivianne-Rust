//! The two dialogs of the Git tab: commit all changes, and open a pull request. Either can have its words written
//! from the changes by the model the thread uses.

use gpui_kit::base::Disableable as _;
use gpui_kit::base::input::{Textarea, TextareaState};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, EventEmitter, Window, div, px};
use serde_json::json;
use vivianne_ai::host::ModelRef;
use vivianne_tools::git::{self, Direction};

use super::ui::{github, muted};
use crate::theme::{self, color};

/// What the dialog is for.
#[derive(Debug, Clone)]
pub enum Kind {
    Commit,
    PullRequest { branch: String, needs_push: bool },
}

#[derive(Debug, Clone)]
pub enum ComposeEvent {
    /// It went through: what to say about it.
    Done(String),
}

pub struct Compose {
    kind: Kind,
    root: String,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    base: Entity<InputState>,
    draft: bool,
    writing: bool,
    saving: bool,
    error: Option<String>,
}

impl EventEmitter<ComposeEvent> for Compose {}

impl Compose {
    pub fn new(
        kind: Kind,
        root: String,
        base: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pr = matches!(kind, Kind::PullRequest { .. });
        let title = cx.new(|cx| {
            InputState::new(window, cx).placeholder(if pr {
                "Title"
            } else {
                "Fix the login timeout"
            })
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(if pr { 6 } else { 3 }, 14)
                .placeholder(if pr {
                    "## What\n- …\n\n## Why\n…\n\n## Testing\n…"
                } else {
                    "Details (optional)"
                })
        });
        let base = cx.new(|cx| InputState::new(window, cx).default_value(base.to_owned()));
        Self {
            kind,
            root,
            title,
            body,
            base,
            draft: false,
            writing: false,
            saving: false,
            error: None,
        }
    }

    /// Opens the dialog over the window; `on_done` hears how it ended.
    pub fn open(
        kind: Kind,
        root: String,
        base: &str,
        window: &mut Window,
        cx: &mut gpui_kit::App,
        on_done: impl Fn(&str, &mut gpui_kit::App) + 'static,
    ) {
        let heading = if matches!(kind, Kind::Commit) {
            "Commit all changes"
        } else {
            "Open a pull request"
        };
        let entity = cx.new(|cx| Self::new(kind, root, base, window, cx));
        cx.subscribe(&entity, move |_compose, ComposeEvent::Done(text), cx| {
            on_done(text, cx)
        })
        .detach();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog.title(heading).w(px(540.0)).child(entity.clone())
        });
    }

    /// Asks the model to write the title and body from the changes (or from the branch's commits).
    fn write(&mut self, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        self.writing = true;
        self.error = None;
        let (root, pr, base) = (
            self.root.clone(),
            matches!(self.kind, Kind::PullRequest { .. }),
            self.base.read(cx).value().to_string(),
        );
        let (view, branch) = (
            cx.entity().downgrade(),
            if let Kind::PullRequest { branch, .. } = &self.kind {
                branch.clone()
            } else {
                String::new()
            },
        );
        let events = crate::backend::get().map(|b| b.events.clone());
        crate::backend::run(
            cx,
            async move {
                let input = git::suggest_input(&root, pr, Some(&base)).await?;
                let selection = vivianne_providers::env::selection()?;
                let model = ModelRef {
                    id: selection.model.clone(),
                    name: selection.model,
                    provider: selection.provider,
                    ..ModelRef::default()
                };
                let events = events.ok_or("The background services did not start.")?;
                let reply = vivianne_ai::ai::completion::complete_background(
                    &events,
                    &model,
                    input.system,
                    &input.user,
                )
                .await?;
                Ok::<_, String>(git::parse_suggestion(&reply, input.is_pr, &branch))
            },
            move |answer, cx| {
                view.update(cx, |this, cx| {
                    this.writing = false;
                    match answer {
                        Ok((title, body)) => this.fill(&title, &body, cx),
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                })
                .ok();
            },
        );
        cx.notify();
    }

    fn fill(&mut self, title: &str, body: &str, cx: &mut Context<Self>) {
        let (title, body) = (title.to_owned(), body.to_owned());
        let (t, b) = (self.title.clone(), self.body.clone());
        // Setting a value needs the window, so it is done on the next frame's update.
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                window
                    .update(cx, |_, window, cx| {
                        t.update(cx, |t, cx| t.set_value(title, window, cx));
                        b.update(cx, |b, cx| b.set_value(body, window, cx));
                    })
                    .ok();
            }
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().trim().to_owned();
        let body = self.body.read(cx).value().trim().to_owned();
        if title.is_empty() || self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        let root = self.root.clone();
        let view = cx.entity().downgrade();
        let _ = window;
        match self.kind.clone() {
            Kind::Commit => {
                let message = if body.is_empty() {
                    title
                } else {
                    format!("{title}\n\n{body}")
                };
                crate::backend::run(
                    cx,
                    async move {
                        git::commit(&root, &message, None)
                            .await
                            .map(|made| format!("Committed {made}"))
                    },
                    move |answer, cx| Self::finish(&view, answer, cx),
                );
            }
            Kind::PullRequest { branch, needs_push } => {
                let (base, draft) = (self.base.read(cx).value().to_string(), self.draft);
                crate::backend::run(
                    cx,
                    async move {
                        if needs_push {
                            git::sync(&root, Direction::Push).await?;
                        }
                        let made = github(root, "/repos/{repo}/pulls".to_owned(), "POST", Some(json!({ "title": title, "body": body, "head": branch, "base": base, "draft": draft })), false).await?;
                        Ok(format!(
                            "Opened pull request #{}",
                            super::ui::n(&made, "number")
                        ))
                    },
                    move |answer, cx| Self::finish(&view, answer, cx),
                );
            }
        }
        cx.notify();
    }

    fn finish(
        view: &gpui_kit::WeakEntity<Self>,
        answer: Result<String, String>,
        cx: &mut gpui_kit::App,
    ) {
        let closing = answer.is_ok();
        view.update(cx, |this, cx| {
            this.saving = false;
            match answer {
                Ok(text) => cx.emit(ComposeEvent::Done(text)),
                Err(e) => this.error = Some(e),
            }
            cx.notify();
        })
        .ok();
        if closing && let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, cx| window.close_dialog(cx))
                .ok();
        }
    }
}

impl Render for Compose {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pr = matches!(self.kind, Kind::PullRequest { .. });
        let title_len = self.title.read(cx).value().chars().count();
        let can_send = title_len > 0 && !self.saving;
        let submit_label = match &self.kind {
            Kind::Commit => "Commit",
            Kind::PullRequest {
                needs_push: true, ..
            } => "Push and open",
            Kind::PullRequest { .. } => "Open pull request",
        };
        let intro = match &self.kind {
            Kind::PullRequest { branch, needs_push } => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font_family(theme::MONO)
                            .text_size(px(theme::TEXT_SM))
                            .child(branch.clone()),
                    )
                    .child(muted("into"))
                    .child(div().w(px(120.0)).child(Input::new(&self.base).small()))
                    .children(needs_push.then(|| muted("· the branch is pushed first"))),
            ),
            Kind::Commit => None,
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(intro)
            .child(Input::new(&self.title))
            .children(
                (!pr && title_len > 72)
                    .then(|| muted(format!("{title_len} characters: keep it under 72"))),
            )
            .child(Textarea::new(&self.body))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        Button::new("compose-write")
                            .ghost()
                            .small()
                            .disabled(self.writing)
                            .label(if self.writing {
                                "Writing…"
                            } else if pr {
                                "Write it from the commits"
                            } else {
                                "Write it from the changes"
                            })
                            .on_click(cx.listener(|this, _e, _w, cx| this.write(cx))),
                    )
                    .children(pr.then(|| {
                        Checkbox::new("compose-draft")
                            .label("Draft")
                            .checked(self.draft)
                            .on_click(cx.listener(|this, checked: &bool, _w, cx| {
                                this.draft = *checked;
                                cx.notify();
                            }))
                    })),
            )
            .children(self.error.clone().map(|e| {
                div()
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::DANGER))
                    .child(e)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        Button::new("compose-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(|_e, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("compose-submit")
                            .primary()
                            .small()
                            .disabled(!can_send)
                            .label(submit_label)
                            .on_click(cx.listener(|this, _e, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
