//! Memory: what Vivianne remembers, how it is kept, and how it is searched.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::Shared;
use crate::settings::widgets::{
    btn, field_dropdown, field_input, field_switch, keyed_input, muted, note, stat, strong,
};

const EMBEDDERS: [(&str, &str); 2] = [("local", "On this computer"), ("cloud", "A cloud provider")];

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Memory")
        .icon(IconName::BookOpen)
        .description("What Vivianne keeps between conversations.")
        .group(behavior_group(values))
        .group(stats_group(values))
        .group(pages_group(values))
        .group(search_group(values))
        .group(embedder_group(values))
        .group(hindsight_group(values))
}

fn behavior_group(values: &Shared) -> SettingGroup {
    SettingGroup::new().items([
        SettingItem::new(
            "Remember across sessions",
            field_switch(
                values,
                |v| v.remember_across_sessions,
                |v, on| v.remember_across_sessions = on,
            ),
        )
        .description("Carry what it learned into the next conversation."),
        SettingItem::new(
            "Tidy up automatically",
            field_switch(
                values,
                |v| v.auto_consolidate,
                |v, on| v.auto_consolidate = on,
            ),
        )
        .description("Merge duplicates and drop what no longer matters while you are away."),
        SettingItem::new(
            "Pick up facts as we go",
            field_switch(values, |v| v.auto_extract, |v, on| v.auto_extract = on),
        )
        .description("Save lasting facts from a conversation without being asked."),
        SettingItem::new(
            "Never keep secrets",
            field_switch(values, |v| v.refuse_secrets, |v, on| v.refuse_secrets = on),
        )
        .description("Refuse to store keys, passwords and card numbers."),
    ])
}

fn stats_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().item(SettingItem::render(move |_options, _window, _cx| {
        let v = values.borrow();
        h_flex()
            .gap_8()
            .child(stat(v.memory_entries.to_string(), "Entries"))
            .child(stat(format!("{}", v.memory_tokens), "Tokens"))
            .child(stat(format!("{:.1} MB", v.memory_mb), "On disk"))
            .into_any_element()
    }))
}

fn pages_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Memory pages")
        .item(note(
            "Each page answers one question from what has been remembered.",
        ))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let pages = values.borrow().memory_pages.clone();
            let rows = pages.into_iter().map(|p| {
                div()
                    .flex()
                    .flex_col()
                    .child(strong(p.name))
                    .child(muted(p.question))
            });
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(rows)
                .into_any_element()
        }))
}

fn search_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Search memory")
        .item(SettingItem::render(move |_options, window, cx| {
            let input = keyed_input(
                "mem-query",
                &values,
                |v| v.mem_query.clone(),
                |v, x| v.mem_query = x,
                "Search what it remembers",
                window,
                cx,
            );
            let (query, hits) = {
                let v = values.borrow();
                (v.mem_query.to_lowercase(), v.mem_results.clone())
            };
            let shown = hits
                .into_iter()
                .filter(|h| query.is_empty() || h.text.to_lowercase().contains(&query));
            let rows = shown.map(|h| {
                div()
                    .flex()
                    .flex_col()
                    .child(strong(h.text))
                    .child(muted(h.date))
            });
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(input)
                .children(rows)
                .into_any_element()
        }))
}

fn embedder_group(values: &Shared) -> SettingGroup {
    let test = values.clone();
    SettingGroup::new()
        .title("Search engine")
        .description("Turns memories into numbers so they can be found by meaning.")
        .item(SettingItem::new(
            "Prefer",
            field_dropdown(
                values,
                &EMBEDDERS,
                |v| v.cloud_prefer.clone(),
                |v, x| v.cloud_prefer = x,
            ),
        ))
        .item(SettingItem::new(
            "Cloud provider",
            field_input(
                values,
                |v| v.cloud_provider.clone(),
                |v, x| v.cloud_provider = x,
            ),
        ))
        .item(SettingItem::new(
            "Cloud model",
            field_input(values, |v| v.cloud_model.clone(), |v, x| v.cloud_model = x),
        ))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let result = test.borrow().cloud_test.clone();
            let values = test.clone();
            h_flex()
                .gap_3()
                .items_center()
                .child(btn("embedder-test", "Test it", move |_, _, cx| {
                    let mut v = values.borrow_mut();
                    v.cloud_test = Some(format!(
                        "{} / {} is set up.",
                        v.cloud_provider, v.cloud_model
                    ));
                    cx.refresh_windows();
                }))
                .children(result.map(muted))
                .into_any_element()
        }))
}

fn hindsight_group(values: &Shared) -> SettingGroup {
    SettingGroup::new()
        .title("Hindsight")
        .description("An outside memory service to use alongside this one.")
        .items([
            SettingItem::new(
                "Use Hindsight",
                field_switch(values, |v| v.hindsight_on, |v, on| v.hindsight_on = on),
            ),
            SettingItem::new(
                "Address",
                field_input(values, |v| v.hs_address.clone(), |v, x| v.hs_address = x),
            ),
            SettingItem::new(
                "Bank",
                field_input(values, |v| v.hs_bank.clone(), |v, x| v.hs_bank = x),
            ),
            SettingItem::new(
                "Key",
                field_input(values, |v| v.hs_key.clone(), |v, x| v.hs_key = x),
            ),
            SettingItem::new(
                "Send new memories",
                field_switch(values, |v| v.hs_send, |v, on| v.hs_send = on),
            ),
        ])
}
