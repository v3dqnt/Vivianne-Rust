//! Diagnostics: sharing usage data, what was recorded, crash reports, and sending a report. The numbers come
//! from `vivianne_telemetry` and the sharing settings from the settings file; nothing here is invented.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, div};
use serde_json::json;
use vivianne_telemetry::{CrashRecord, Report, Summary};

use super::account::civil;
use crate::backend;
use crate::settings::values::{Crash, Shared};
use crate::settings::widgets::{btn, confirm, keyed_textarea, mono, muted, note, strong};

/// Longest stack trace shown, in lines.
const TRACE_LINES: usize = 60;

pub fn page(values: &Shared) -> SettingPage {
    start_loading(values);
    SettingPage::new("Diagnostics")
        .icon(IconName::Info)
        .description("What Vivianne records about itself, and how to report a problem.")
        .group(performance_group())
        .group(sharing_group(values))
        .group(recorded_group(values))
        .group(crashes_group(values))
        .group(report_group(values))
}

// ---------------------------------------------------------------- loading ---

fn start_loading(values: &Shared) {
    {
        let v = values.borrow();
        if v.diag_loaded || v.diag_loading {
            return;
        }
    }
    values.borrow_mut().diag_loading = true;
    LOAD_REQUEST.with(|slot| *slot.borrow_mut() = Some(values.clone()));
}

thread_local! {
    static LOAD_REQUEST: std::cell::RefCell<Option<Shared>> = const { std::cell::RefCell::new(None) };
}

/// Starts the read asked for by [`start_loading`]; called from a draw, which has an `App`.
fn run_pending_load(cx: &mut App) {
    let Some(values) = LOAD_REQUEST.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    load(&values, cx);
}

/// Reads the sharing settings and what is recorded.
fn load(values: &Shared, cx: &mut App) {
    let values = values.clone();
    backend::run(
        cx,
        async {
            let settings = vivianne_ai::host::read_settings_json().await;
            let summary = tokio::task::spawn_blocking(vivianne_telemetry::summary)
                .await
                .unwrap_or_default();
            (settings, summary)
        },
        move |(settings, summary), cx| {
            {
                let mut v = values.borrow_mut();
                v.share_diagnostics = settings
                    .get("telemetryEnabled")
                    .and_then(|s| s.as_bool())
                    .unwrap_or(false);
                v.share_endpoint = settings
                    .get("telemetryEndpoint")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_owned();
                show_summary(&mut v, &summary);
                v.diag_loaded = true;
                v.diag_loading = false;
            }
            cx.refresh_windows();
        },
    );
}

fn show_summary(v: &mut crate::settings::values::Values, summary: &Summary) {
    v.diag_events = u32::try_from(summary.event_count).unwrap_or(u32::MAX);
    v.diag_since = summary.first_event_ts.map(time_text).unwrap_or_default();
    v.diag_kinds = summary
        .kinds
        .iter()
        .map(|(kind, count)| (kind.clone(), u32::try_from(*count).unwrap_or(u32::MAX)))
        .collect();
    v.crashes = summary.crashes.iter().map(crash_row).collect();
    v.open_crash = None;
}

/// `YYYY-MM-DD HH:MM` (UTC) for a time in Unix milliseconds.
fn time_text(ms: u64) -> String {
    let secs = ms / 1000;
    let (y, m, d) = civil(i64::try_from(secs / 86_400).unwrap_or(0));
    let minute_of_day = (secs % 86_400) / 60;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        minute_of_day / 60,
        minute_of_day % 60
    )
}

fn crash_row(crash: &CrashRecord) -> Crash {
    let mut trace = vec![
        format!("at {}", crash.location),
        format!("version {} on {}", crash.app_version, crash.os),
    ];
    trace.extend(crash.backtrace.lines().take(TRACE_LINES).map(str::to_owned));
    Crash {
        time: time_text(crash.ts),
        kind: crash.message.clone(),
        trace,
    }
}

/// Reads what is recorded again (after Clear).
fn reload_summary(values: &Shared, cx: &mut App) {
    let values = values.clone();
    backend::run(
        cx,
        async {
            tokio::task::spawn_blocking(vivianne_telemetry::summary)
                .await
                .unwrap_or_default()
        },
        move |summary, cx| {
            show_summary(&mut values.borrow_mut(), &summary);
            cx.refresh_windows();
        },
    );
}

// ---------------------------------------------------------------- sharing ---

/// Saves one sharing setting, leaving the rest of the settings file as it is.
fn save_setting(key: &'static str, value: serde_json::Value, cx: &mut App) {
    backend::run(
        cx,
        async move { vivianne_ai::host::update_settings(move |s| s[key] = value).await },
        |result, _cx| {
            if let Err(why) = result {
                eprintln!("vivianne: could not save the setting: {why}");
            }
        },
    );
}

fn sharing_group(values: &Shared) -> SettingGroup {
    let (switch_read, switch_write, input_read, input_write) = (
        values.clone(),
        values.clone(),
        values.clone(),
        values.clone(),
    );
    let switch = SettingField::switch(
        move |_| switch_read.borrow().share_diagnostics,
        move |on, cx| {
            switch_write.borrow_mut().share_diagnostics = on;
            save_setting("telemetryEnabled", json!(on), cx);
            cx.refresh_windows();
        },
    );
    let endpoint = SettingField::input(
        move |_| SharedString::from(input_read.borrow().share_endpoint.clone()),
        move |value, cx| {
            let text = value.trim().to_owned();
            input_write.borrow_mut().share_endpoint = value.to_string();
            save_setting("telemetryEndpoint", json!(text), cx);
            cx.refresh_windows();
        },
    );
    SettingGroup::new()
        .item(
            SettingItem::new("Share bug reports", switch)
                .description("Off by default. When on, a report you send is also uploaded to the address below."),
        )
        .item(
            SettingItem::new("Send to", endpoint)
                .description("An https:// address. Leave empty, or keep sharing off, to keep everything on this computer."),
        )
}

// --------------------------------------------------------------- recorded ---

fn recorded_group(values: &Shared) -> SettingGroup {
    let (shown, clearer) = (values.clone(), values.clone());
    SettingGroup::new()
        .title("Recorded here")
        .item(SettingItem::render(move |_options, _window, cx| {
            run_pending_load(cx);
            let v = shown.borrow();
            if !v.diag_loaded {
                return muted("Reading what was recorded…").into_any_element();
            }
            let rows = v.diag_kinds.iter().map(|(kind, count)| h_flex().justify_between().child(mono(kind.clone())).child(muted(count.to_string())));
            let heading = if v.diag_events == 0 {
                "Nothing recorded yet.".to_owned()
            } else {
                format!("{} events since {}", v.diag_events, v.diag_since)
            };
            div().flex().flex_col().gap_1().child(strong(heading)).children(rows).into_any_element()
        }))
        .item(note("Only counts and names: which kind of thing happened, never what you wrote."))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let clearer = clearer.clone();
            btn("diag-clear", "Clear recorded data", move |_, window, cx| {
                let clearer = clearer.clone();
                confirm(
                    window,
                    cx,
                    "Delete recorded data?",
                    "This deletes every recorded event and crash on this computer. It cannot be undone.",
                    "Delete",
                    true,
                    move |_window, cx| {
                        let values = clearer.clone();
                        backend::run(cx, async { tokio::task::spawn_blocking(vivianne_telemetry::clear).await.unwrap_or_else(|e| Err(e.to_string())) }, move |result, cx| {
                            if let Err(why) = result {
                                values.borrow_mut().report_result = Some(format!("Could not delete: {why}"));
                            }
                            reload_summary(&values, cx);
                        });
                    },
                );
            })
            .danger()
            .into_any_element()
        }))
}

// ---------------------------------------------------------------- crashes ---

fn crashes_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Crashes")
        .item(SettingItem::render(move |_options, _window, _cx| {
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
                        v.open_crash = if v.open_crash == Some(index) {
                            None
                        } else {
                            Some(index)
                        };
                        cx.refresh_windows();
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(strong(crash.time.clone()))
                            .child(toggle),
                    )
                    .child(muted(crash.kind.clone()))
                    .children(is_open.then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .children(crash.trace.iter().map(|l| mono(l.clone())))
                    }))
            });
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(rows)
                .into_any_element()
        }))
}

// ----------------------------------------------------------------- report ---

fn report_group(values: &Shared) -> SettingGroup {
    let (form, result) = (values.clone(), values.clone());
    SettingGroup::new()
        .title("Report a problem")
        .item(note("Say what happened. The report is saved on this computer first; it is only uploaded if sharing is on."))
        .item(SettingItem::render(move |_options, window, cx| {
            let input = keyed_textarea("report-text", &form, |v| v.report_text.clone(), |v, x| v.report_text = x, "What went wrong?", (3, 8), window, cx);
            let attach = crate::settings::widgets::toggle("report-attach", &form, |v| v.report_attach, |v, on| v.report_attach = on);
            let sender = form.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(input)
                .child(h_flex().gap_2().items_center().child(attach).child(muted("Attach recent events and the last crash")))
                .child(btn("report-send", "Send", move |_, _, cx| send_report(&sender, cx)))
                .into_any_element()
        }))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let text = result.borrow().report_result.clone();
            div().children(text.map(muted)).into_any_element()
        }))
}

fn send_report(values: &Shared, cx: &mut App) {
    let (text, attach) = {
        let mut v = values.borrow_mut();
        if v.report_sending {
            return;
        }
        if v.report_text.trim().is_empty() {
            v.report_result = Some("Write a few words first.".to_owned());
            cx.refresh_windows();
            return;
        }
        v.report_sending = true;
        v.report_result = Some("Saving…".to_owned());
        (v.report_text.clone(), v.report_attach)
    };
    cx.refresh_windows();
    let values = values.clone();
    backend::run(
        cx,
        async move { vivianne_telemetry::submit_report(&text, attach).await },
        move |result, cx| {
            {
                let mut v = values.borrow_mut();
                v.report_sending = false;
                v.report_result = Some(report_outcome(&result));
            }
            cx.refresh_windows();
        },
    );
}

/// What Send says: where the report was saved, and whether it was uploaded.
fn report_outcome(result: &Result<Report, String>) -> String {
    match result {
        Err(why) => why.clone(),
        Ok(report) => {
            let saved = format!("Saved to {}.", report.path.display());
            match (&report.upload_error, report.uploaded) {
                (_, true) => format!("{saved} Uploaded."),
                (Some(why), false) => format!("{saved} Not uploaded: {why}."),
                (None, false) => {
                    format!("{saved} Not uploaded: sharing is off or no https:// address is set.")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn report(uploaded: bool, error: Option<&str>) -> Result<Report, String> {
        Ok(Report {
            path: PathBuf::from("r.json"),
            uploaded,
            upload_error: error.map(str::to_owned),
        })
    }

    #[test]
    fn send_says_where_the_report_went_and_whether_it_was_uploaded() {
        assert!(report_outcome(&report(true, None)).ends_with("Uploaded."));
        assert!(
            report_outcome(&report(false, Some("no route"))).contains("Not uploaded: no route")
        );
        assert!(report_outcome(&report(false, None)).contains("sharing is off"));
        assert!(report_outcome(&report(false, None)).starts_with("Saved to r.json"));
        assert_eq!(report_outcome(&Err("boom".to_owned())), "boom");
    }

    #[test]
    fn a_time_is_shown_as_a_utc_date_and_minute() {
        assert_eq!(time_text(0), "1970-01-01 00:00 UTC");
        assert_eq!(time_text(1_763_600_000_000), "2025-11-20 00:53 UTC");
    }
}

/// Frame pace of the whole window and the app's log, shown live.
fn performance_group() -> SettingGroup {
    SettingGroup::new()
        .title("Performance")
        .description("How smoothly the app is drawing, and what its background parts are doing.")
        .item(SettingItem::render(|_options, window, cx| {
            let view = window.use_keyed_state("diag-performance", cx, |_window, cx| {
                crate::rail::performance::PerformanceView::new(cx)
            });
            div().h(gpui_kit::px(420.0)).child(view).into_any_element()
        }))
}
