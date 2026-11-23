//! The Debug tab: the program the agent (or you) is debugging through a debug adapter. Controls, call stack,
//! variables, breakpoints and the program's output. It draws what `vivianne_tools::dap::ui` reports for this
//! project's session (one per workspace folder) and sends the buttons back to it. Nothing here starts a program:
//! that is `debug_run`, which asks first.

use gpui_kit::base::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{Context, FontWeight, SharedString, Window, div, px};
use serde_json::Value;
use vivianne_tools::dap::ui;

use crate::theme::{self, color};

/// How often the session is read while the tab exists.
const POLL_MS: u64 = 500;

pub struct DebuggerView {
    thread: String,
    snapshot: Value,
    frame: Option<i64>,
}

impl DebuggerView {
    pub fn new(root: &std::path::Path, cx: &mut Context<Self>) -> Self {
        let thread = root.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(POLL_MS))
                    .await;
                let (thread, frame) = match this.read_with(cx, |v, _| (v.thread.clone(), v.frame)) {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let Some(rx) =
                    crate::backend::spawn(
                        async move { ui::cmd_debug_snapshot(thread, frame).await },
                    )
                else {
                    continue;
                };
                if let Ok(Ok(snapshot)) = rx.await
                    && this
                        .update(cx, |v, cx| {
                            if v.snapshot != snapshot {
                                v.snapshot = snapshot;
                                cx.notify();
                            }
                        })
                        .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            thread,
            snapshot: Value::Null,
            frame: None,
        }
    }

    fn control(
        &self,
        action: &'static str,
        file: Option<String>,
        line: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let thread = self.thread.clone();
        crate::backend::run(
            cx,
            async move { ui::cmd_debug_control(thread, action.to_owned(), file, line).await },
            |_, _| {},
        );
    }
}

fn text(s: impl Into<SharedString>, size: f32, shade: u32) -> gpui_kit::Div {
    div()
        .text_size(px(size))
        .text_color(color(shade))
        .child(s.into())
}

fn heading(label: &str) -> gpui_kit::Div {
    div()
        .text_size(px(theme::TEXT_XS))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(theme::MUTED))
        .child(label.to_uppercase())
}

fn dot(shade: u32) -> gpui_kit::Div {
    div().size(px(8.0)).rounded_full().bg(color(shade))
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The file's name without its folders.
fn short(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn variable_rows(vars: &[Value], depth: usize, out: &mut Vec<gpui_kit::AnyElement>) {
    for v in vars {
        let value = str_of(v, "value").replace('\n', " ");
        out.push(
            div()
                .flex()
                .gap(px(8.0))
                .pl(px(12.0 * depth as f32))
                .font_family(theme::MONO)
                .text_size(px(11.5))
                .child(
                    div()
                        .flex_none()
                        .text_color(color(theme::ACCENT_TEXT))
                        .child(str_of(v, "name").to_owned()),
                )
                .child(div().min_w_0().text_color(color(theme::TEXT)).child(value))
                .into_any_element(),
        );
        if let Some(children) = v.get("children").and_then(Value::as_array) {
            variable_rows(children, depth + 1, out);
        }
    }
}

impl Render for DebuggerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snapshot.clone();
        let shell = || {
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .px(px(20.0))
                .py(px(14.0))
                .overflow_hidden()
        };
        if snap.is_null() {
            return shell()
                .child(text("Nothing is being debugged", theme::TEXT_BASE, theme::TEXT))
                .child(text(
                    "Ask the agent to debug a program: it launches it under a debug adapter (debugpy, lldb-dap, gdb, delve and others), sets breakpoints and steps through it. You will be asked before anything starts, and the session shows up here.",
                    theme::TEXT_SM,
                    theme::MUTED,
                ));
        }
        let status = str_of(&snap, "status").to_owned();
        let stopped = status == "stopped";
        let running = status == "running" || status == "starting";
        let live = stopped || running;
        let (state_shade, state_label) = match status.as_str() {
            "stopped" => (theme::WARN, "Paused"),
            "running" => (theme::OK, "Running"),
            "starting" => (theme::ACCENT, "Starting"),
            _ => (theme::FAINT, "Ended"),
        };
        let reason = str_of(&snap, "reason").to_owned();
        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(dot(state_shade))
                    .child(text(state_label, theme::TEXT_BASE, theme::TEXT))
                    .children(
                        (!reason.is_empty()).then(|| text(reason, theme::TEXT_SM, theme::MUTED)),
                    )
                    .children(
                        snap.get("exitCode")
                            .and_then(Value::as_i64)
                            .map(|c| text(format!("exit code {c}"), theme::TEXT_SM, theme::MUTED)),
                    ),
            )
            .child(text(
                format!(
                    "{} · {}",
                    str_of(&snap, "adapter"),
                    short(str_of(&snap, "program"))
                ),
                theme::TEXT_XS,
                theme::FAINT,
            ));

        let button = |id: &'static str, label: &'static str, action: &'static str, on: bool| {
            Button::new(id)
                .ghost()
                .small()
                .label(label)
                .disabled(!on)
                .on_click(cx.listener(move |view, _e, _w, cx| view.control(action, None, None, cx)))
        };
        let controls = div()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .child(button("dbg-continue", "Continue", "continue", stopped))
            .child(button("dbg-pause", "Pause", "pause", running))
            .child(button("dbg-over", "Step over", "step_over", stopped))
            .child(button("dbg-in", "Step in", "step_in", stopped))
            .child(button("dbg-out", "Step out", "step_out", stopped))
            .child(button("dbg-restart", "Restart", "restart", live))
            .child(
                Button::new("dbg-stop")
                    .outline()
                    .small()
                    .label(if live { "Stop" } else { "Dismiss" })
                    .on_click(cx.listener(|view, _e, _w, cx| {
                        view.control("stop", None, None, cx);
                        view.snapshot = Value::Null;
                        cx.notify();
                    })),
            );

        let selected = snap
            .get("selectedFrame")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let frames = snap
            .get("frames")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let stack = frames.iter().map(|f| {
            let id = f.get("id").and_then(Value::as_i64).unwrap_or(0);
            let here = id == selected;
            div()
                .id(("dbg-frame", usize::try_from(id).unwrap_or(0)))
                .flex()
                .gap(px(8.0))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .when(here, |d| d.bg(color(theme::ACCENT_SOFT)))
                .hover(|d| d.bg(color(theme::RAISED)))
                .text_size(px(12.0))
                .child(
                    div()
                        .text_color(color(theme::TEXT))
                        .child(str_of(f, "name").to_owned()),
                )
                .child(div().text_color(color(theme::FAINT)).child(format!(
                    "{}:{}",
                    short(str_of(f, "file")),
                    f.get("line").and_then(Value::as_i64).unwrap_or(0)
                )))
                .on_click(cx.listener(move |view, _e, _w, cx| {
                    view.frame = Some(id);
                    cx.notify();
                }))
        });

        let mut var_rows = Vec::new();
        for scope in snap
            .get("scopes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            var_rows.push(
                heading(str_of(scope, "name"))
                    .pt(px(4.0))
                    .into_any_element(),
            );
            let vars = scope
                .get("variables")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            variable_rows(&vars, 0, &mut var_rows);
        }

        let exception = snap.get("exception").filter(|e| !e.is_null()).map(|e| {
            div()
                .p(px(10.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .bg(color(theme::SURFACE))
                .child(text(
                    format!("{}: {}", str_of(e, "type"), str_of(e, "message")),
                    theme::TEXT_SM,
                    theme::DANGER,
                ))
        });

        let breakpoints = snap
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let bp_rows = breakpoints.iter().enumerate().map(|(i, b)| {
            let file = str_of(b, "file").to_owned();
            let line = b.get("line").and_then(Value::as_u64).unwrap_or(0);
            let verified = b.get("verified").and_then(Value::as_bool).unwrap_or(false);
            let label = if line == 0 {
                file.clone()
            } else {
                format!("{}:{line}", short(&file))
            };
            let remove_file = file.clone();
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(dot(if verified {
                    theme::DANGER
                } else {
                    theme::FAINT
                }))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(color(theme::TEXT))
                        .child(label),
                )
                .child(
                    Button::new(("dbg-bp", i))
                        .ghost()
                        .small()
                        .label("Remove")
                        .on_click(cx.listener(move |view, _e, _w, cx| {
                            view.control(
                                "remove_breakpoint",
                                Some(remove_file.clone()),
                                u32::try_from(line).ok(),
                                cx,
                            );
                        })),
                )
        });

        let output = str_of(&snap, "output").to_owned();

        shell()
            .child(header)
            .child(controls)
            .children(exception)
            .child(
                div()
                    .id("dbg-body")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(heading("Call stack"))
                            .children(stopped.then(|| div().flex().flex_col().children(stack)))
                            .children((!stopped).then(|| {
                                text("Available while paused.", theme::TEXT_SM, theme::FAINT)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(heading("Variables"))
                            .children(var_rows),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(heading("Breakpoints"))
                            .children(
                                (breakpoints.is_empty())
                                    .then(|| text("None set.", theme::TEXT_SM, theme::FAINT)),
                            )
                            .children(bp_rows),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(heading("Output"))
                            .child(
                                div()
                                    .font_family(theme::MONO)
                                    .text_size(px(11.5))
                                    .text_color(color(theme::TEXT))
                                    .child(if output.is_empty() {
                                        "Nothing printed yet.".to_owned()
                                    } else {
                                        output
                                    }),
                            ),
                    ),
            )
    }
}
