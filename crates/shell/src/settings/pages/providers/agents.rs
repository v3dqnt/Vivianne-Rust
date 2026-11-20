//! Coding agents: other agents (Claude Code, Codex, Gemini CLI...) run through the Agent Client Protocol.
//! The list is the protocol's public registry; each one starts through Node (`npx`) or uv (`uvx`) and
//! downloads the first time it is set up.

use gpui_kit::base::h_flex;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, div};
use vivianne_providers::acp::{self, AuthKind, AuthMethod, AuthState};

use crate::backend;
use crate::settings::provider_state::{AgentAuth, AgentInfo, AgentReport};
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
    let (busy, report, auth, progress) = {
        let v = values.borrow();
        (
            v.providers.is_busy(&format!("agent-{}", agent.id)),
            v.providers.agent_reports.get(&agent.id).cloned(),
            v.providers.agent_auth.get(&agent.id).cloned(),
            v.providers.agent_progress.get(&agent.id).cloned(),
        )
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
    if agent.available && agent.ready {
        let (values, id) = (values.clone(), agent.id.clone());
        let checking = matches!(auth, Some(AgentAuth::Checking));
        actions = actions.child(Button::new(SharedString::from(format!("auth-{}", agent.id))).label(if checking { "Checking…" } else { "Check sign-in" }).small().on_click(
            move |_, _, cx| {
                if !checking {
                    check_sign_in(&values, &id, cx);
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
        .children(progress.filter(|_| busy).map(muted))
        .children(auth.map(|a| auth_view(values, &agent.id, &a)))
        .children(report.map(|r| report_view(&r)))
}

/// Whether the agent is signed in, and if not, how to sign it in.
fn auth_view(values: &Shared, id: &str, auth: &AgentAuth) -> gpui_kit::AnyElement {
    match auth {
        AgentAuth::Checking => muted("Checking whether it is signed in…").into_any_element(),
        AgentAuth::SignedIn => div().text_sm().text_color(theme::color(theme::OK)).child("Signed in. It is ready to use.").into_any_element(),
        AgentAuth::Failed(why) => div().text_sm().text_color(theme::color(theme::DANGER)).child(why.clone()).into_any_element(),
        AgentAuth::Needs(methods) => {
            let rows: Vec<_> = methods.iter().map(|m| method_row(values, id, m)).collect();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().text_color(theme::color(theme::DANGER)).child("It needs you to sign in before it can be used."))
                .children(rows)
                .into_any_element()
        }
    }
}

fn method_row(values: &Shared, id: &str, method: &AuthMethod) -> gpui_kit::AnyElement {
    let (values, agent_id, method_id) = (values.clone(), id.to_owned(), method.id.clone());
    match (&method.kind, &method.command) {
        (AuthKind::Terminal, Some(command)) => {
            let (copy, again, text) = (command.clone(), values.clone(), command.clone());
            let agent = agent_id.clone();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(muted("Run this in a terminal, then check again:"))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(mono(text))
                        .child(Button::new(SharedString::from(format!("copy-{agent_id}-{method_id}"))).label("Copy").small().ghost().on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy.clone()));
                        }))
                        .child(Button::new(SharedString::from(format!("again-{agent_id}-{method_id}"))).label("Check again").small().on_click(move |_, _, cx| {
                            check_sign_in(&again, &agent, cx);
                        })),
                )
                .into_any_element()
        }
        _ => {
            let label = format!("Sign in: {}", method.name);
            Button::new(SharedString::from(format!("signin-{agent_id}-{method_id}")))
                .label(label)
                .small()
                .on_click(move |_, _, cx| sign_in(&values, &agent_id, &method_id, cx))
                .into_any_element()
        }
    }
}

/// Asks the agent whether it is signed in.
fn check_sign_in(values: &Shared, id: &str, cx: &mut App) {
    values.borrow_mut().providers.agent_auth.insert(id.to_owned(), AgentAuth::Checking);
    cx.refresh_windows();
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { acp::check_auth(&ask).await }, move |result, cx| {
        store_auth(&values, &id, result);
        cx.refresh_windows();
    });
}

/// Signs the agent in the way it offered, which may open a browser, then shows where that left it.
fn sign_in(values: &Shared, id: &str, method: &str, cx: &mut App) {
    values.borrow_mut().providers.agent_auth.insert(id.to_owned(), AgentAuth::Checking);
    cx.refresh_windows();
    let (values, id, method) = (values.clone(), id.to_owned(), method.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { acp::sign_in(&ask, &method).await }, move |result, cx| {
        store_auth(&values, &id, result);
        cx.refresh_windows();
    });
}

fn store_auth(values: &Shared, id: &str, result: Result<AuthState, String>) {
    let auth = match result {
        Ok(AuthState::SignedIn) => AgentAuth::SignedIn,
        Ok(AuthState::NeedsSignIn(methods)) => AgentAuth::Needs(methods),
        Ok(AuthState::Failed(why)) | Err(why) => AgentAuth::Failed(why),
    };
    values.borrow_mut().providers.agent_auth.insert(id.to_owned(), auth);
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
    let slot = id.clone();
    watch_progress(values.clone(), id.clone(), cx);
    backend::run(cx, async move { acp::install(&ask, &move |p| store_progress(&slot, &p)).await }, move |result, cx| {
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

/// What an install last reported, by agent, written from the async side and read while the page redraws.
static PROGRESS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[allow(clippy::cast_precision_loss)]
fn store_progress(id: &str, progress: &acp::InstallProgress) {
    let megabytes = progress.bytes as f64 / 1_048_576.0;
    let text = format!("{} ({megabytes:.0} MB on disk)", progress.line);
    let mut all = PROGRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    all.retain(|(known, _)| known != id);
    all.push((id.to_owned(), text));
}

/// Copies what the install reports into the page every second, until it is done.
fn watch_progress(values: Shared, id: String, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let still = values.borrow().providers.is_busy(&format!("agent-{id}"));
            let text = PROGRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(known, _)| *known == id).map(|(_, t)| t.clone());
            if let Some(text) = text {
                values.borrow_mut().providers.agent_progress.insert(id.clone(), text);
            }
            cx.update(|cx| cx.refresh_windows());
            if !still {
                break;
            }
        }
    })
    .detach();
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
