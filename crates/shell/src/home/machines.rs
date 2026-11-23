//! The Machines tab of Home: this computer, then each saved machine with its threads, its terminal panes and what
//! its agents are doing. A machine that is down keeps its last list, dimmed, with a note (cached state); one that
//! needs the person carries `! auth` or `! error`.
//!
//! From here a prompt can be run on a machine (the model key is sent for that connection only, never stored there),
//! a terminal pane opened, and an approval a remote run is waiting for answered.

use std::collections::HashMap;

use gpui_kit::base::{Disableable as _, h_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, SharedString, Window, div, px};
use vivianne_remote::client::{Hub, View};
use vivianne_remote::protocol::{AgentState, ClientMessage, WireEvent};

use super::HomeView;
use crate::theme::{self, color};

/// The inputs of one machine's block.
pub struct MachineUi {
    workspace: Entity<InputState>,
    prompt: Entity<InputState>,
    command: Entity<InputState>,
    line: Entity<InputState>,
}

/// An id that is not guessable and does not repeat, for a thread or a pane. Not a secret.
fn fresh_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:x}-{:x}", std::process::id())
}

fn muted(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(color(theme::MUTED))
        .child(text.into())
}

/// The latest run of a thread on a machine, as text: what it said and, if it is waiting, what it asks.
fn run_text(view: &View, thread_id: &str) -> (String, Option<(String, String)>) {
    let mut text = String::new();
    let mut ask = None;
    for (id, event) in &view.activity {
        if id != thread_id {
            continue;
        }
        match event {
            WireEvent::Text { text: piece } => text.push_str(piece),
            WireEvent::Approval { id, title } => ask = Some((id.clone(), title.clone())),
            WireEvent::ToolStart { name, summary } => {
                text.push_str(&format!("\n[{name}] {summary}\n"));
                ask = None;
            }
            WireEvent::Done | WireEvent::Stopped | WireEvent::Failed { .. } => ask = None,
            _ => {}
        }
    }
    let tail: String = text
        .chars()
        .rev()
        .take(1200)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (tail, ask)
}

impl HomeView {
    fn machine_ui(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> &MachineUi {
        self.machine_ui
            .entry(id.to_owned())
            .or_insert_with(|| MachineUi {
                workspace: cx
                    .new(|cx| InputState::new(window, cx).placeholder("folder on that machine")),
                prompt: cx.new(|cx| {
                    InputState::new(window, cx).placeholder("Ask the agent on this machine")
                }),
                command: cx
                    .new(|cx| InputState::new(window, cx).placeholder("program to run (optional)")),
                line: cx.new(|cx| {
                    InputState::new(window, cx).placeholder("type into the pane, Enter to send")
                }),
            })
    }

    pub(super) fn machines(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut column = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_color(color(theme::MUTED))
                .child("Other computers you reach over SSH. Add one in Settings, Machines."),
        );
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().child("This computer"))
                .child(muted(format!(
                    "{} open thread{}",
                    self.threads.len(),
                    if self.threads.len() == 1 { "" } else { "s" }
                ))),
        );
        if self.machine_views.is_empty() {
            return column.child(muted("No other machines saved."));
        }
        let views = self.machine_views.clone();
        for (i, v) in views.iter().enumerate() {
            let block = self.machine_block(i, v, window, cx);
            column = column.child(block);
        }
        column
    }

    fn machine_block(
        &mut self,
        index: usize,
        v: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = v.machine.id.clone();
        let open = !self.folded.contains(&id);
        let cached = v.is_cached();
        let badge = v.badge();
        let fold_id = id.clone();
        let header = div()
            .id(("machine", index))
            .flex()
            .items_center()
            .justify_between()
            .h(px(36.0))
            .px(px(12.0))
            .rounded(px(theme::RADIUS_CONTROL))
            .cursor_pointer()
            .hover(|style| style.bg(color(theme::SURFACE)))
            .on_click(cx.listener(move |home, _e, _w, cx| {
                if !home.folded.remove(&fold_id) {
                    home.folded.insert(fold_id.clone());
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(div().child(v.machine.label.clone()))
                    .children((!badge.is_empty()).then(|| div().child(badge))),
            )
            .child(muted(v.status_text()));
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .opacity(if cached { 0.6 } else { 1.0 })
            .child(header);
        if !open {
            return block;
        }
        if !cached {
            let controls = self.controls(&v.machine.id, window, cx);
            block = block.child(controls);
        }
        if v.threads.is_empty() {
            block = block.child(div().pl(px(24.0)).child(muted("No threads")));
        }
        for t in v.threads.iter().take(30) {
            let state = v
                .runs
                .iter()
                .find(|r| r.thread_id == t.id)
                .map_or("Saved", |r| match r.state {
                    AgentState::Working => "Working",
                    AgentState::Blocked => "Waiting for you",
                    AgentState::Idle | AgentState::Unknown => "Idle",
                });
            let state = if cached {
                format!("{state} · last seen")
            } else {
                state.to_owned()
            };
            block = block.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(32.0))
                    .pl(px(24.0))
                    .pr(px(12.0))
                    .child(div().child(t.title.clone()))
                    .child(muted(format!("{} · {state}", v.machine.label))),
            );
        }
        // What the machine's runs have said lately, and the approval one of them is waiting for.
        let mut seen = Vec::new();
        for (thread_id, _) in v.activity.iter().rev() {
            if !seen.contains(thread_id) {
                seen.push(thread_id.clone());
            }
            if seen.len() == 2 {
                break;
            }
        }
        for thread_id in seen {
            let (text, ask) = run_text(v, &thread_id);
            block = block.child(self.run_view(&v.machine.id, &thread_id, &text, ask, cached, cx));
        }
        for pane in &v.panes {
            block = block.child(self.pane_view(v, &pane.id, cached, cx));
        }
        block
    }

    /// The prompt and terminal controls of a machine that is online.
    fn controls(
        &mut self,
        machine: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ui = self.machine_ui(machine, window, cx);
        let (workspace, prompt, command) =
            (ui.workspace.clone(), ui.prompt.clone(), ui.command.clone());
        let (run_ws, run_prompt, run_machine) =
            (workspace.clone(), prompt.clone(), machine.to_owned());
        let (pane_ws, pane_cmd, pane_machine) =
            (workspace.clone(), command.clone(), machine.to_owned());
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pl(px(24.0))
            .pr(px(12.0))
            .py(px(6.0))
            .child(h_flex().gap(px(6.0)).child(Input::new(&workspace)))
            .child(
                h_flex().gap(px(6.0)).child(Input::new(&prompt)).child(
                    Button::new(SharedString::from(format!("run-{machine}")))
                        .primary()
                        .label("Run")
                        .on_click(cx.listener(move |_home, _e, window, cx| {
                            let (ws, text) = (
                                run_ws.read(cx).value().to_string(),
                                run_prompt.read(cx).value().to_string(),
                            );
                            if text.trim().is_empty() {
                                return;
                            }
                            run_prompt_on(&run_machine, ws.trim(), text.trim());
                            run_prompt.update(cx, |input, cx| input.set_value("", window, cx));
                        })),
                ),
            )
            .child(
                h_flex().gap(px(6.0)).child(Input::new(&command)).child(
                    Button::new(SharedString::from(format!("pane-{machine}")))
                        .outline()
                        .label("New terminal")
                        .on_click(cx.listener(move |_home, _e, _w, cx| {
                            let ws = pane_ws.read(cx).value().to_string();
                            let cmd = pane_cmd.read(cx).value().to_string();
                            Hub::global().send(
                                &pane_machine,
                                ClientMessage::PaneOpen {
                                    pane: format!("p{}", fresh_id()),
                                    workspace: ws.trim().to_owned(),
                                    command: (!cmd.trim().is_empty())
                                        .then(|| cmd.trim().to_owned()),
                                    cols: 100,
                                    rows: 24,
                                },
                            );
                        })),
                ),
            )
    }

    /// One run's latest words and, when it waits, the question with Allow and Deny.
    fn run_view(
        &self,
        machine: &str,
        thread_id: &str,
        text: &str,
        ask: Option<(String, String)>,
        cached: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .pl(px(24.0))
            .pr(px(12.0))
            .py(px(6.0))
            .child(muted(format!("Run {thread_id}")))
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_size(px(11.0))
                    .child(text.to_owned()),
            );
        if let (Some((id, title)), false) = (ask, cached) {
            let answer = |allow: bool,
                          label: &'static str,
                          key: &'static str,
                          machine: String,
                          thread_id: String,
                          id: String| {
                Button::new(SharedString::from(format!("{key}-{id}")))
                    .outline()
                    .label(label)
                    .on_click(cx.listener(move |_h, _e, _w, _cx| {
                        Hub::global().send(
                            &machine,
                            ClientMessage::ApprovalAnswer {
                                thread_id: thread_id.clone(),
                                id: id.clone(),
                                allow,
                            },
                        );
                    }))
            };
            block = block
                .child(div().child(format!("Waiting for you: {title}")))
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(answer(
                            true,
                            "Allow",
                            "allow",
                            machine.to_owned(),
                            thread_id.to_owned(),
                            id.clone(),
                        ))
                        .child(answer(
                            false,
                            "Deny",
                            "deny",
                            machine.to_owned(),
                            thread_id.to_owned(),
                            id,
                        )),
                );
        }
        block
    }

    /// A terminal pane: its state, the screen as text, and a line to type into it.
    fn pane_view(
        &self,
        v: &View,
        pane: &str,
        cached: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let info = v.panes.iter().find(|p| p.id == pane);
        let state = info.map_or("", |p| match p.state {
            AgentState::Working => "working",
            AgentState::Blocked => "waiting for you",
            AgentState::Idle => "idle",
            AgentState::Unknown => "",
        });
        let agent = info.and_then(|p| p.agent.clone()).unwrap_or_default();
        let lines = v
            .frames
            .get(pane)
            .map(|(l, _)| l.join("\n"))
            .unwrap_or_default();
        let (machine, close_pane) = (v.machine.id.clone(), pane.to_owned());
        let line = self.machine_ui.get(&v.machine.id).map(|ui| ui.line.clone());
        let (send_machine, send_pane) = (v.machine.id.clone(), pane.to_owned());
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .pl(px(24.0))
            .pr(px(12.0))
            .py(px(6.0))
            .child(
                h_flex()
                    .justify_between()
                    .child(muted(format!("Terminal {pane} {agent} {state}")))
                    .child(
                        Button::new(SharedString::from(format!("close-{pane}")))
                            .ghost()
                            .label("Close")
                            .disabled(cached)
                            .on_click(cx.listener(move |_h, _e, _w, _cx| {
                                Hub::global().send(
                                    &machine,
                                    ClientMessage::PaneClose {
                                        pane: close_pane.clone(),
                                    },
                                );
                            })),
                    ),
            );
        block = block.child(
            div()
                .font_family(theme::MONO)
                .text_size(px(11.0))
                .bg(color(theme::SURFACE))
                .rounded(px(theme::RADIUS_CONTROL))
                .p(px(8.0))
                .child(if lines.is_empty() {
                    "(no screen yet; it is sent while this tab is open)".to_owned()
                } else {
                    lines
                }),
        );
        if let (Some(line), false) = (line, cached) {
            block = block.child(
                h_flex().gap(px(6.0)).child(Input::new(&line)).child(
                    Button::new(SharedString::from(format!("send-{pane}")))
                        .outline()
                        .label("Send")
                        .on_click(cx.listener(move |_h, _e, window, cx| {
                            let text = line.read(cx).value().to_string();
                            Hub::global().send(
                                &send_machine,
                                ClientMessage::PaneInput {
                                    pane: send_pane.clone(),
                                    data: format!("{text}\r"),
                                },
                            );
                            line.update(cx, |input, cx| input.set_value("", window, cx));
                        })),
                ),
            );
        }
        block
    }
}

/// Starts a prompt on a machine: the model key for this connection, then the run. A thread has a fresh id.
fn run_prompt_on(machine: &str, workspace: &str, prompt: &str) {
    let Ok(selection) = vivianne_providers::env::selection() else {
        vivianne_core::debuglog::log(
            "remote",
            "no model is chosen, so nothing was run on the machine",
        );
        return;
    };
    let hub = Hub::global();
    if let Some(key) = vivianne_providers::env::key_for(&selection.provider) {
        hub.send(
            machine,
            ClientMessage::SessionCredential {
                provider: selection.provider.clone(),
                key,
            },
        );
    }
    hub.send(
        machine,
        ClientMessage::RunPrompt {
            thread_id: fresh_id(),
            workspace: workspace.to_owned(),
            prompt: prompt.to_owned(),
            provider: selection.provider,
            model: selection.model,
            mode: "ask".to_owned(),
        },
    );
}

/// The inputs kept per machine, by machine id.
pub type Inputs = HashMap<String, MachineUi>;
