//! Machines: other computers this app works with over SSH. Each runs a Vivianne server that owns its threads and
//! agent runs, so work continues when this app closes; this page saves the machines, shows how each connection is
//! and reconnects it. Authentication stays with OpenSSH: no password is asked for or kept here.

use std::sync::Mutex;

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, Styled, div};
use vivianne_remote::catalog::Catalog;
use vivianne_remote::client::{Hub, View};

use crate::settings::values::Shared;
use crate::settings::widgets::{keyed_input, mono, muted, strong};

const TARGET: &str = "machine-target";
const LABEL: &str = "machine-label";

/// The last problem with adding or changing a machine, shown under the form.
static PROBLEM: Mutex<String> = Mutex::new(String::new());

fn set_problem(text: String) {
    *PROBLEM
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = text;
}

fn problem() -> String {
    PROBLEM
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The connections change on their own, so the page looks again a moment after something is done.
fn refresh_soon(cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(1500))
            .await;
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

pub fn page(values: &Shared) -> SettingPage {
    let add = values.clone();
    SettingPage::new("Machines")
        .icon(IconName::Globe)
        .description("Other computers you reach over SSH. Each runs its own Vivianne server, so its work keeps going when this app is closed or the network drops. Sign-in, host keys and jump hosts stay with OpenSSH and your ~/.ssh/config.")
        .group(
            SettingGroup::new()
                .title("Add a machine")
                .description("An ssh alias, user@host, or ssh://user@host:2222. Never a password. The remote needs `viv` installed: run `viv machine install <label>` from a terminal to put it there (it asks first).")
                .item(SettingItem::render(move |_o, window, cx| {
                    let values = add.clone();
                    let target = keyed_input(
                        TARGET,
                        &values,
                        |v| v.key_drafts.get(TARGET).cloned().unwrap_or_default(),
                        |v, x| {
                            v.key_drafts.insert(TARGET.to_owned(), x);
                        },
                        "workbox, or me@host",
                        window,
                        cx,
                    );
                    let label = keyed_input(
                        LABEL,
                        &values,
                        |v| v.key_drafts.get(LABEL).cloned().unwrap_or_default(),
                        |v, x| {
                            v.key_drafts.insert(LABEL.to_owned(), x);
                        },
                        "label (optional)",
                        window,
                        cx,
                    );
                    let pressed = values.clone();
                    let error = problem();
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            h_flex().gap_2().child(target).child(label).child(
                                Button::new("machine-add")
                                    .small()
                                    .label("Add")
                                    .on_click(move |_e, window, cx| {
                                        let (target, label) = {
                                            let v = pressed.borrow();
                                            (
                                                v.key_drafts.get(TARGET).cloned().unwrap_or_default(),
                                                v.key_drafts.get(LABEL).cloned().unwrap_or_default(),
                                            )
                                        };
                                        let result = Catalog::load().and_then(|mut c| {
                                            let label = (!label.trim().is_empty()).then_some(label.trim());
                                            c.add(target.trim(), label, None)?;
                                            c.save()
                                        });
                                        match result {
                                            Ok(()) => {
                                                set_problem(String::new());
                                                pressed.borrow_mut().key_drafts.remove(TARGET);
                                                pressed.borrow_mut().key_drafts.remove(LABEL);
                                                crate::backend::sync_machines();
                                                refresh_soon(cx);
                                            }
                                            Err(e) => set_problem(e),
                                        }
                                        window.refresh();
                                        cx.refresh_windows();
                                    }),
                            ),
                        )
                        .children((!error.is_empty()).then(|| muted(error)))
                        .into_any_element()
                })),
        )
        .group(
            SettingGroup::new()
                .title("Saved machines")
                .item(SettingItem::render(|_o, _w, _cx| list().into_any_element())),
        )
}

fn list() -> impl IntoElement {
    let views = Hub::global().views();
    if views.is_empty() {
        return muted("No machines yet.").into_any_element();
    }
    div()
        .flex()
        .flex_col()
        .gap_4()
        .children(views.iter().map(row))
        .into_any_element()
}

fn row(v: &View) -> impl IntoElement + use<> {
    let id = v.machine.id.clone();
    let (reconnect_id, toggle_id, remove_id) = (id.clone(), id.clone(), id);
    let enabled = v.machine.enabled;
    let runs = v.runs.len();
    let summary = format!(
        "{} thread{}{}",
        v.threads.len(),
        if v.threads.len() == 1 { "" } else { "s" },
        if runs == 0 {
            String::new()
        } else {
            format!(", {runs} running")
        }
    );
    let badge = v.badge();
    let mut text = div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .child(
            h_flex()
                .gap_2()
                .child(strong(v.machine.label.clone()))
                .children((!badge.is_empty()).then(|| strong(badge))),
        )
        .child(mono(format!(
            "{}  ·  session {}",
            v.machine.target, v.machine.session
        )))
        .child(muted(v.status_text()));
    if !v.is_cached() || !v.threads.is_empty() {
        let note = if v.is_cached() {
            format!("{summary} (last seen)")
        } else {
            summary
        };
        text = text.child(muted(note));
    }
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .opacity(if v.is_cached() { 0.75 } else { 1.0 })
        .child(text)
        .child(
            div()
                .flex()
                .gap_1()
                .flex_none()
                .child(
                    Button::new(SharedString::from(format!(
                        "machine-reconnect-{reconnect_id}"
                    )))
                    .outline()
                    .small()
                    .label("Reconnect")
                    .on_click(move |_e, window, cx| {
                        Hub::global().retry_now(&reconnect_id);
                        refresh_soon(cx);
                        window.refresh();
                    }),
                )
                .child(
                    Button::new(SharedString::from(format!("machine-toggle-{toggle_id}")))
                        .outline()
                        .small()
                        .label(if enabled { "Turn off" } else { "Turn on" })
                        .on_click(move |_e, window, cx| {
                            let _ = Catalog::load().and_then(|mut c| {
                                c.set_enabled(&toggle_id, !enabled)?;
                                c.save()
                            });
                            crate::backend::sync_machines();
                            refresh_soon(cx);
                            window.refresh();
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("machine-remove-{remove_id}")))
                        .outline()
                        .danger()
                        .small()
                        .label("Remove")
                        .on_click(move |_e, window, cx| {
                            // Only this app's link goes; what runs on the machine keeps running.
                            let _ = Catalog::load().and_then(|mut c| {
                                c.remove(&remove_id)?;
                                c.save()
                            });
                            crate::backend::sync_machines();
                            window.refresh();
                            cx.refresh_windows();
                        }),
                ),
        )
}
