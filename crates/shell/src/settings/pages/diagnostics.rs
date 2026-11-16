//! Diagnostics: sharing usage data, what was recorded, crash reports, and sending a report.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::Shared;
use crate::settings::widgets::{btn, field_input, field_switch, keyed_textarea, mono, muted, note, strong};

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Diagnostics")
        .icon(IconName::Info)
        .description("What Vivianne records about itself, and how to report a problem.")
        .group(sharing_group(values))
        .group(recorded_group(values))
        .group(crashes_group(values))
        .group(report_group(values))
}

fn sharing_group(values: &Shared) -> SettingGroup {
    SettingGroup::new()
        .item(
            SettingItem::new(
                "Share anonymous diagnostics",
                field_switch(values, |v| v.share_diagnostics, |v, on| v.share_diagnostics = on),
            )
            .description("Counts of what happened, never what you wrote. Off by default."),
        )
        .item(
            SettingItem::new("Send to", field_input(values, |v| v.share_endpoint.clone(), |v, x| v.share_endpoint = x))
                .description("Leave empty to keep everything on this computer."),
        )
}

fn recorded_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().title("Recorded here").item(SettingItem::render(move |_options, _window, _cx| {
        let (events, since, kinds) = {
            let v = values.borrow();
            (v.diag_events, v.diag_since.clone(), v.diag_kinds.clone())
        };
        let rows = kinds.into_iter().map(|(kind, count)| {
            h_flex().justify_between().child(mono(kind)).child(muted(count.to_string()))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(strong(format!("{events} events since {since}")))
            .children(rows)
            .into_any_element()
    }))
}

fn crashes_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().title("Crashes").item(SettingItem::render(move |_options, _window, _cx| {
        let (crashes, open) = {
            let v = values.borrow();
            (v.crashes.clone(), v.open_crash)
        };
        if crashes.is_empty() {
            return muted("No crashes recorded.").into_any_element();
        }
        let rows = crashes.into_iter().enumerate().map(|(index, crash)| {
            let is_open = open == Some(index);
            let values = values.clone();
            let toggle = Button::new(("crash", index))
                .label(if is_open { "Hide" } else { "Show" })
                .ghost()
                .small()
                .on_click(move |_, _, cx| {
                    let mut v = values.borrow_mut();
                    v.open_crash = if v.open_crash == Some(index) { None } else { Some(index) };
                    cx.refresh_windows();
                });
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(h_flex().justify_between().items_center().child(strong(crash.time.clone())).child(toggle))
                .child(muted(crash.kind.clone()))
                .children(is_open.then(|| div().flex().flex_col().children(crash.trace.iter().map(|l| mono(l.clone())))))
        });
        div().flex().flex_col().gap_3().children(rows).into_any_element()
    }))
}

fn report_group(values: &Shared) -> SettingGroup {
    let (form, result) = (values.clone(), values.clone());
    SettingGroup::new()
        .title("Report a problem")
        .item(note("Say what happened. Nothing is sent until you press Send."))
        .item(SettingItem::render(move |_options, window, cx| {
            let input = keyed_textarea(
                "report-text",
                &form,
                |v| v.report_text.clone(),
                |v, x| v.report_text = x,
                "What went wrong?",
                (3, 8),
                window,
                cx,
            );
            let attach = crate::settings::widgets::toggle("report-attach", &form, |v| v.report_attach, |v, on| v.report_attach = on);
            let sender = form.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(input)
                .child(h_flex().gap_2().items_center().child(attach).child(muted("Attach the recent log")))
                .child(btn("report-send", "Send", move |_, _, cx| {
                    let mut v = sender.borrow_mut();
                    v.report_result = Some(report_outcome(&v.report_text));
                    cx.refresh_windows();
                }))
                .into_any_element()
        }))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let text = result.borrow().report_result.clone();
            div().children(text.map(muted)).into_any_element()
        }))
}

/// What Send says. There is no report endpoint yet, so it never claims to have sent anything.
fn report_outcome(text: &str) -> String {
    if text.trim().is_empty() {
        "Write a few words first.".to_owned()
    } else {
        "Reporting is not connected yet; your note stayed on this computer.".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_report_asks_for_words_and_none_claims_to_be_sent() {
        assert_eq!(report_outcome("  "), "Write a few words first.");
        assert!(report_outcome("it broke").contains("not connected"));
    }
}
