//! Small pieces the Git tab's views share: rows, marks, section headings, and reading GitHub's JSON.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Hsla, SharedString, Stateful, Window,
    div, px, rgb,
};
use serde_json::Value;

use crate::theme::{self, color};

/// A string field of GitHub's JSON, or empty.
pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn n(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// A string at a path of fields: `at(pr, &["user", "login"])`.
pub fn at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(v, |v, key| v.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

pub fn list(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(theme::TEXT_SM))
        .text_color(color(theme::MUTED))
        .child(text.into())
}

pub fn faint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(theme::TEXT_XS))
        .text_color(color(theme::FAINT))
        .child(text.into())
}

pub fn mono(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(theme::MONO)
        .text_size(px(theme::TEXT_SM))
        .child(text.into())
}

pub fn dot(hex: u32) -> Div {
    div()
        .size(px(8.0))
        .flex_none()
        .rounded_full()
        .bg(color(hex))
}

/// A pull request's mark: purple merged, red closed, grey draft, green open.
pub fn pr_mark(pr: &Value) -> Div {
    let hex = if !s(pr, "merged_at").is_empty() {
        MERGED
    } else if s(pr, "state") == "closed" {
        theme::DANGER
    } else if pr.get("draft").and_then(Value::as_bool).unwrap_or(false) {
        theme::FAINT
    } else {
        theme::OK
    };
    dot(hex)
}

pub const MERGED: u32 = 0x00a3_71f7;

/// A run, job, step or check as one mark: done well, failed, running or waiting.
pub fn status_mark(status: &str, conclusion: &str) -> AnyElement {
    if !status.is_empty() && status != "completed" {
        return if status == "in_progress" {
            div()
                .size(px(14.0))
                .flex_none()
                .child(Spinner::new().xsmall().color(Hsla::from(rgb(theme::WARN))))
                .into_any_element()
        } else {
            glyph("○", theme::FAINT)
        };
    }
    match conclusion {
        "success" => glyph("✓", theme::OK),
        "failure" | "timed_out" | "startup_failure" => glyph("✕", theme::DANGER),
        _ => glyph("–", theme::FAINT),
    }
}

fn glyph(text: &'static str, hex: u32) -> AnyElement {
    div()
        .w(px(14.0))
        .flex_none()
        .flex()
        .justify_center()
        .text_size(px(theme::TEXT_SM))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(hex))
        .child(text)
        .into_any_element()
}

/// A small filter choice: flat when picked, outlined when not.
pub fn pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(20.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .max_w(px(200.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .rounded(px(4.0))
        .border_1()
        .border_color(color(if active { theme::RAISED } else { theme::BORDER }))
        .bg(color(if active { theme::RAISED } else { theme::PANEL }))
        .text_size(px(theme::TEXT_XS))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(color(if active { theme::TEXT } else { theme::MUTED }))
        .cursor_pointer()
        .hover(|style| style.text_color(color(theme::TEXT)))
        .on_click(on_click)
        .child(label.into())
}

/// A row of a list on the left of a split: icon, title, meta line, optional mark at the right.
pub fn list_row(
    id: impl Into<ElementId>,
    active: bool,
    icon: AnyElement,
    title: impl Into<SharedString>,
    meta: impl Into<SharedString>,
    right: Option<AnyElement>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .flex()
        .items_start()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(6.0))
        .border_b_1()
        .border_color(color(theme::HAIRLINE))
        .bg(color(if active {
            theme::ACCENT_SOFT
        } else {
            theme::PANEL
        }))
        .cursor_pointer()
        .hover(|style| {
            style.bg(color(if active {
                theme::ACCENT_SOFT
            } else {
                theme::SURFACE
            }))
        })
        .on_click(on_click)
        .child(
            div()
                .h(px(20.0))
                .flex()
                .items_center()
                .flex_none()
                .child(icon),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(color(theme::MUTED))
                        .truncate()
                        .child(meta.into()),
                ),
        )
        .children(right.map(|r| div().h(px(20.0)).flex().items_center().flex_none().child(r)))
}

/// A list on the left, the picked item on the right.
pub fn split(
    list: impl IntoElement,
    detail: Option<AnyElement>,
    empty: impl Into<SharedString>,
    id: &'static str,
) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .child(
            div()
                .id((id, 0_usize))
                .w(px(320.0))
                .h_full()
                .flex_none()
                .overflow_y_scroll()
                .border_r_1()
                .border_color(color(theme::HAIRLINE))
                .child(list),
        )
        .child(
            div()
                .id((id, 1_usize))
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_y_scroll()
                .child(
                    detail.unwrap_or_else(|| {
                        div().p(px(16.0)).child(muted(empty)).into_any_element()
                    }),
                ),
        )
}

/// The head of a detail: title, a line under it, the actions and a link to GitHub.
pub fn detail_head(
    title: impl Into<SharedString>,
    sub: impl IntoElement,
    actions: Vec<AnyElement>,
    url: Option<String>,
) -> Div {
    div()
        .px(px(16.0))
        .py(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .border_b_1()
        .border_color(color(theme::HAIRLINE))
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_wrap()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .flex_wrap()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(color(theme::MUTED))
                        .child(sub),
                )
                .children(actions)
                .children(url.map(|url| {
                    Button::new("open-on-github")
                        .ghost()
                        .xsmall()
                        .label("Open on GitHub")
                        .on_click(move |_event, _window, cx| cx.open_url(&url))
                })),
        )
}

/// A titled block of a detail.
pub fn block(title: impl Into<SharedString>, children: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .pb(px(4.0))
                .text_size(px(theme::TEXT_XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(theme::MUTED))
                .child(title.into().to_uppercase()),
        )
        .children(children)
}

pub fn problem(error: &str) -> Div {
    div()
        .p(px(12.0))
        .text_size(px(theme::TEXT_SM))
        .text_color(color(theme::DANGER))
        .child(error.to_owned())
}

/// One call to GitHub for the folder's repository, run on the service runtime.
pub async fn github(
    root: String,
    path: String,
    method: &'static str,
    body: Option<Value>,
    raw: bool,
) -> Result<Value, String> {
    let saved = vivianne_ai::host::stored_api_key("github");
    vivianne_tools::git::github::request(saved, &root, &path, method, body.as_ref(), raw).await
}

/// Runs one GitHub call and hands the answer to the view that asked, on the UI thread.
pub fn call<V: 'static>(
    cx: &mut gpui_kit::Context<V>,
    root: &str,
    path: String,
    method: &'static str,
    body: Option<Value>,
    done: impl FnOnce(&mut V, Result<Value, String>, &mut gpui_kit::Context<V>) + 'static,
) {
    let view = cx.entity().downgrade();
    crate::backend::run(
        cx,
        github(root.to_owned(), path, method, body, false),
        move |answer, cx| {
            view.update(cx, |view, cx| done(view, answer, cx)).ok();
        },
    );
}

/// An action button's progress, error, and the second click a destructive one asks for.
#[derive(Debug, Clone, Default)]
pub struct Act {
    pub busy: Option<&'static str>,
    pub confirming: Option<&'static str>,
    pub error: Option<String>,
}

impl Act {
    /// A click on `label`. True when it should run now; a destructive one asks to be clicked again first.
    pub fn click(&mut self, label: &'static str, confirm: bool) -> bool {
        if self.busy.is_some() {
            return false;
        }
        self.error = None;
        if confirm && self.confirming != Some(label) {
            self.confirming = Some(label);
            return false;
        }
        self.confirming = None;
        self.busy = Some(label);
        true
    }

    pub fn done(&mut self, result: &Result<Value, String>) {
        self.busy = None;
        self.error = result.as_ref().err().cloned();
    }

    /// The button, and the error beside it when the last try failed.
    pub fn button(
        &self,
        id: impl Into<ElementId>,
        label: &'static str,
        danger: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let text = if self.busy == Some(label) {
            "…".to_owned()
        } else if self.confirming == Some(label) {
            format!("Sure? {label}")
        } else {
            label.to_owned()
        };
        let mut button = Button::new(id)
            .outline()
            .xsmall()
            .label(text)
            .on_click(on_click);
        if danger {
            button = button.danger();
        }
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .children(self.error.clone().map(|e| {
                div()
                    .max_w(px(280.0))
                    .truncate()
                    .text_size(px(theme::TEXT_XS))
                    .text_color(color(theme::DANGER))
                    .child(e)
            }))
            .child(button)
            .into_any_element()
    }
}

/// What a load of something from GitHub is doing.
#[derive(Debug, Clone, Default)]
pub enum Load<T> {
    #[default]
    Loading,
    Done(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn from(result: Result<T, String>) -> Self {
        match result {
            Ok(v) => Self::Done(v),
            Err(e) => Self::Failed(e),
        }
    }

    pub fn data(&self) -> Option<&T> {
        match self {
            Self::Done(v) => Some(v),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Failed(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_json_is_read_without_panicking_on_gaps() {
        let v: Value = serde_json::json!({ "title": "x", "user": { "login": "me" }, "n": 3 });
        assert_eq!(
            (
                s(&v, "title"),
                s(&v, "missing"),
                at(&v, &["user", "login"]),
                at(&v, &["user", "x", "y"]),
                n(&v, "n")
            ),
            ("x", "", "me", "", 3)
        );
        assert!(list(&v).is_empty());
    }
}
