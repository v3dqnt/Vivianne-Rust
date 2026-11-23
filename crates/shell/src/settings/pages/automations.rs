//! Automations: prompts that run by themselves, on a schedule or when a file appears in a folder. They are made by
//! asking the agent ("every weekday at nine, summarise what changed in the repo"); this page shows them and switches
//! them on and off.

use chrono::{DateTime, Local};
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, Styled, div};
use vivianne_runtime::automation::{self, Automation};

use crate::settings::values::Shared;
use crate::settings::widgets::{mono, muted, strong};

pub fn page(_values: &Shared) -> SettingPage {
    SettingPage::new("Automations")
        .icon(IconName::Calendar)
        .description("Prompts that run on their own. Ask the agent to make one, for example: \"every weekday at 9, summarise what changed in this repo\".")
        .group(
            SettingGroup::new().title("Scheduled").item(SettingItem::render(|_options, _window, _cx| {
                let items = automation::list();
                if items.is_empty() {
                    return muted("No automations yet.").into_any_element();
                }
                div().flex().flex_col().gap_4().children(items.iter().map(row)).into_any_element()
            })),
        )
}

/// An epoch time as the user's own clock reads it.
fn when(ms: Option<u64>) -> String {
    ms.and_then(|ms| DateTime::from_timestamp_millis(i64::try_from(ms).ok()?))
        .map_or_else(
            || "never".to_owned(),
            |t| {
                t.with_timezone(&Local)
                    .format("%a %d %b, %H:%M")
                    .to_string()
            },
        )
}

fn row(a: &Automation) -> impl IntoElement + use<> {
    let trigger = a.folder.as_ref().map_or_else(
        || {
            format!(
                "cron {} · next {}",
                a.cron,
                if a.enabled {
                    when(a.next_run)
                } else {
                    "off".to_owned()
                }
            )
        },
        |folder| {
            format!(
                "when a file is new in {folder}{}",
                a.glob
                    .as_ref()
                    .map(|g| format!(" ({g})"))
                    .unwrap_or_default()
            )
        },
    );
    let (toggle_id, delete_id) = (a.id.clone(), a.id.clone());
    let enabled = a.enabled;
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w_0()
                .child(strong(a.name.clone()))
                .child(mono(trigger))
                .child(muted(a.prompt.clone()))
                .child(muted(format!("last run {}", when(a.last_run)))),
        )
        .child(
            div()
                .flex()
                .gap_1()
                .flex_none()
                .child(
                    Button::new(SharedString::from(format!("automation-toggle-{}", a.id)))
                        .outline()
                        .small()
                        .label(if enabled { "Turn off" } else { "Turn on" })
                        .on_click(move |_event, window, cx| {
                            let _ = automation::set_enabled(&toggle_id, !enabled);
                            window.refresh();
                            cx.refresh_windows();
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("automation-delete-{}", a.id)))
                        .outline()
                        .danger()
                        .small()
                        .label("Delete")
                        .on_click(move |_event, window, cx| {
                            let _ = automation::delete(&delete_id);
                            window.refresh();
                            cx.refresh_windows();
                        }),
                ),
        )
}
