//! Coding agents: other agents (Claude Code, Codex, Gemini CLI...) run through the Agent Client Protocol.
//! The list is the protocol's public registry; each one starts through Node (`npx`) or uv (`uvx`) and
//! downloads the first time it is set up.

use gpui_kit::base::h_flex;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, div};
use vivianne_providers::acp;

use crate::backend;
use crate::settings::provider_state::{AgentInfo, AgentReport};
use crate::settings::values::Shared;
use crate::settings::widgets::{mono, muted, note, pill, strong};
use crate::theme;

pub fn group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Coding agents")
        .item(note("Other coding agents, run through the Agent Client Protocol. A thread can use one from the model picker. Each starts through Node (npx) or uv (uvx) and downloads the first time it is set up."))
        .item(SettingItem::render(move |_options, _window, cx| {
            load_once(&values, cx);
            view(&values).into_any_element()
        }))
}

/// Reads the registry the first time the group is drawn.
fn load_once(values: &Shared, cx: &mut App) {
    {
        let mut v = values.borrow_mut();
        if v.providers.agents.is_some() || v.providers.is_busy("agents") {
            return;
        }
        v.providers.start("agents");
    }
    let values = values.clone();
    backend::run(cx, acp::harnesses(), move |found, cx| {
        {
            let mut v = values.borrow_mut();
            v.providers.agents = Some(found.into_iter().map(info).collect());
            v.providers.finish("agents");
        }
        cx.refresh_windows();
    });
}

fn info(h: acp::Harness) -> AgentInfo {
    AgentInfo { id: h.id, name: h.name, description: h.description, launcher: h.launcher, available: h.available, ready: h.ready }
}

fn view(values: &Shared) -> impl IntoElement + use<> {
    let (agents, log) = {
        let v = values.borrow();
        (v.providers.agents.clone(), v.providers.agent_log.clone())
    };
    let Some(agents) = agents else {
        return muted("Looking up coding agents…").into_any_element();
    };
    let launcher_missing = agents.iter().any(|a| !a.available);
    let rows: Vec<_> = agents.iter().map(|a| row(values, a)).collect();
    let (show, reset) = (values.clone(), values.clone());
    let buttons = h_flex()
        .gap_2()
        .child(Button::new("agents-log").label("Show log").small().on_click(move |_, _, cx| {
            show.borrow_mut().providers.agent_log = Some(acp::trace_lines());
            cx.refresh_windows();
        }))
        .child(Button::new("agents-reset").label("Stop running agents").small().ghost().on_click(move |_, _, cx| {
            if let Some(backend) = backend::get() {
                backend.acp.reset();
            }
            reset.borrow_mut().providers.agent_log = Some(acp::trace_lines());
            cx.refresh_windows();
        }));
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(launcher_missing.then(|| {
            div()
                .text_sm()
                .text_color(theme::color(theme::DANGER))
                .child("Node.js or uv isn't installed on this computer, so the agents marked Not installed can't start. Install Node.js from nodejs.org, then reopen this page.")
        }))
        .children(rows)
        .child(buttons)
        .children(log.map(|lines| log_box(&lines)))
        .into_any_element()
}

fn status(agent: &AgentInfo) -> &'static str {
    if !agent.available {
        "Not installed"
    } else if agent.ready {
        "Ready"
    } else {
        "Needs setup"
    }
}

fn row(values: &Shared, agent: &AgentInfo) -> impl IntoElement + use<> {
    let (busy, report) = {
        let v = values.borrow();
        (v.providers.is_busy(&format!("agent-{}", agent.id)), v.providers.agent_reports.get(&agent.id).cloned())
    };
    let mut actions = h_flex().gap_2();
    if agent.available && !agent.ready {
        let (values, id) = (values.clone(), agent.id.clone());
        actions = actions.child(Button::new(SharedString::from(format!("setup-{}", agent.id))).label(if busy { "Setting up…" } else { "Set up" }).small().on_click(
            move |_, _, cx| {
                if !busy {
                    set_up(&values, &id, cx);
                }
            },
        ));
    }
    if agent.available {
        let (values, id) = (values.clone(), agent.id.clone());
        actions = actions.child(Button::new(SharedString::from(format!("test-{}", agent.id))).label("Test").small().ghost().on_click(move |_, _, cx| {
            if !busy {
                test(&values, &id, cx);
            }
        }));
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme::color(theme::BORDER))
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .gap_3()
                .child(div().flex().flex_col().child(strong(agent.name.clone())).child(muted(agent.description.clone())))
                .child(h_flex().gap_2().items_center().child(pill(status(agent))).child(actions)),
        )
        .children(report.map(|r| report_view(&r)))
}

fn report_view(report: &AgentReport) -> impl IntoElement + use<> {
    let steps: Vec<_> = report
        .steps
        .iter()
        .map(|(name, ok, seconds, detail)| {
            let mark = if *ok { "✓" } else { "✗" };
            let line = div().text_sm().text_color(theme::color(if *ok { theme::MUTED } else { theme::DANGER })).child(format!("{mark} {name} ({seconds:.1}s): {detail}"));
            line.into_any_element()
        })
        .collect();
    div().flex().flex_col().gap_1().children(steps).children((!report.reply.is_empty()).then(|| muted(format!("It said: \"{}\"", report.reply))))
}

fn log_box(lines: &[String]) -> impl IntoElement + use<> {
    let text = if lines.is_empty() { "Nothing logged yet.".to_owned() } else { lines.join("\n") };
    div().p_2().rounded_md().bg(theme::color(theme::CODE_BG)).max_h(gpui_kit::px(220.0)).overflow_hidden().child(mono(text))
}

fn set_up(values: &Shared, id: &str, cx: &mut App) {
    let key = format!("agent-{id}");
    values.borrow_mut().providers.start(&key);
    cx.refresh_windows();
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { acp::install(&ask).await }, move |result, cx| {
        {
            let mut v = values.borrow_mut();
            v.providers.finish(&key);
            if let Err(why) = result {
                v.providers.error = Some(why);
            }
        }
        // Whether it is ready now is in the registry's marks.
        values.borrow_mut().providers.agents = None;
        let _ = id;
        cx.refresh_windows();
    });
}

fn test(values: &Shared, id: &str, cx: &mut App) {
    let key = format!("agent-{id}");
    {
        let mut v = values.borrow_mut();
        v.providers.start(&key);
        v.providers.agent_reports.remove(id);
    }
    cx.refresh_windows();
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { acp::debug(&ask, None).await }, move |result, cx| {
        {
            let mut v = values.borrow_mut();
            v.providers.finish(&key);
            match result {
                Ok(report) => {
                    let steps = report.steps.into_iter().map(|s| (s.name, s.ok, millis_to_seconds(s.millis), s.detail)).collect();
                    v.providers.agent_reports.insert(id, AgentReport { steps, reply: report.reply });
                    v.providers.agent_log = Some(report.trace);
                }
                Err(why) => v.providers.error = Some(why),
            }
        }
        cx.refresh_windows();
    });
}

#[allow(clippy::cast_precision_loss)]
fn millis_to_seconds(millis: u64) -> f64 {
    millis as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(available: bool, ready: bool) -> AgentInfo {
        AgentInfo { id: "a".into(), name: "A".into(), description: String::new(), launcher: "npx".into(), available, ready }
    }

    #[test]
    fn an_agent_is_missing_until_its_launcher_is_here_then_needs_setup_until_it_has_run() {
        assert_eq!(status(&agent(false, false)), "Not installed");
        assert_eq!(status(&agent(true, false)), "Needs setup");
        assert_eq!(status(&agent(true, true)), "Ready");
    }
}
