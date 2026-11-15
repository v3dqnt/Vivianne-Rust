//! The Settings pages, in the old app's order (Viv Instant is left out on purpose).

use gpui_kit::component::IconName;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, div};

use super::values::{Shared, Values};
use crate::theme::{self, color};

const NOT_BUILT: &str = "This page is not built yet.";

pub fn all(values: &Shared) -> Vec<SettingPage> {
    vec![
        account(),
        providers(),
        local_models(),
        reliability(values),
        mods(),
        connections(),
        web_and_browser(values),
        memory(values),
        vault(),
        skills(),
        diagnostics(values),
        appearance(values),
        layout(values),
        components(),
        updates(),
    ]
}

fn page(title: &str, icon: IconName, group_description: &str) -> SettingPage {
    SettingPage::new(title.to_owned())
        .icon(icon)
        .description(group_description.to_owned())
}

/// A line of plain text inside a group.
fn note(text: &'static str) -> SettingItem {
    SettingItem::render(move |_options, _window, _cx| {
        div()
            .text_color(color(theme::MUTED))
            .child(text)
            .into_any_element()
    })
}

fn placeholder_page(title: &str, icon: IconName, group_description: &str) -> SettingPage {
    page(title, icon, group_description).group(SettingGroup::new().item(note(NOT_BUILT)))
}

fn switch(
    values: &Shared,
    get: fn(&Values) -> bool,
    set: fn(&mut Values, bool),
) -> SettingField<bool> {
    let (read, write) = (values.clone(), values.clone());
    SettingField::switch(
        move |_| get(&read.borrow()),
        move |on, _| set(&mut write.borrow_mut(), on),
    )
}

fn dropdown(
    values: &Shared,
    options: &[(&str, &str)],
    get: fn(&Values) -> &String,
    set: fn(&mut Values, String),
) -> SettingField<SharedString> {
    let options = options
        .iter()
        .map(|(value, label)| {
            (
                SharedString::from(value.to_owned()),
                SharedString::from(label.to_owned()),
            )
        })
        .collect();
    let (read, write) = (values.clone(), values.clone());
    SettingField::dropdown(
        options,
        move |_| SharedString::from(get(&read.borrow()).clone()),
        move |value, _| set(&mut write.borrow_mut(), value.to_string()),
    )
}

fn account() -> SettingPage {
    placeholder_page("Account & usage", IconName::User, "Account")
}

fn providers() -> SettingPage {
    placeholder_page("Providers", IconName::Cpu, "Model")
}

fn local_models() -> SettingPage {
    placeholder_page("Local models", IconName::HardDrive, "Model")
}

fn reliability(values: &Shared) -> SettingPage {
    page("Reliability", IconName::CircleCheck, "Model").group(
        SettingGroup::new().title("Failures").item(
            SettingItem::new(
                "Retry transient failures",
                switch(
                    values,
                    |v| v.retry_transient,
                    |v, on| v.retry_transient = on,
                ),
            )
            .description("Try again when a provider is briefly unavailable."),
        ),
    )
}

fn mods() -> SettingPage {
    placeholder_page("Mods", IconName::Frame, "Agent")
}

fn connections() -> SettingPage {
    placeholder_page("Connections", IconName::Network, "Agent")
}

fn web_and_browser(values: &Shared) -> SettingPage {
    page("Web & browser", IconName::Globe, "Agent").group(
        SettingGroup::new()
            .title("Web search")
            .item(SettingItem::new(
                "Search provider",
                dropdown(
                    values,
                    &[
                        ("auto", "Automatic"),
                        ("brave", "Brave"),
                        ("tavily", "Tavily"),
                        ("exa", "Exa"),
                        ("duckduckgo", "DuckDuckGo"),
                    ],
                    |v| &v.search_provider,
                    |v, value| v.search_provider = value,
                ),
            )),
    )
}

fn memory(values: &Shared) -> SettingPage {
    page("Memory", IconName::MemoryStick, "Agent").group(
        SettingGroup::new()
            .title("What Vivianne remembers")
            .item(SettingItem::new(
                "Remember across sessions",
                switch(
                    values,
                    |v| v.remember_across_sessions,
                    |v, on| v.remember_across_sessions = on,
                ),
            ))
            .item(SettingItem::new(
                "Auto-consolidate",
                switch(
                    values,
                    |v| v.auto_consolidate,
                    |v, on| v.auto_consolidate = on,
                ),
            ))
            .item(SettingItem::new(
                "Auto-extraction",
                switch(values, |v| v.auto_extract, |v, on| v.auto_extract = on),
            ))
            .item(SettingItem::new(
                "Refuse secrets",
                switch(values, |v| v.refuse_secrets, |v, on| v.refuse_secrets = on),
            )),
    )
}

fn vault() -> SettingPage {
    placeholder_page("Vault", IconName::Eye, "Agent")
}

fn skills() -> SettingPage {
    placeholder_page("Skills", IconName::BookOpen, "Agent")
}

fn diagnostics(values: &Shared) -> SettingPage {
    page("Diagnostics", IconName::Inspector, "App").group(
        SettingGroup::new().item(
            SettingItem::new(
                "Share anonymous diagnostics",
                switch(
                    values,
                    |v| v.share_diagnostics,
                    |v, on| v.share_diagnostics = on,
                ),
            )
            .description("Off by default."),
        ),
    )
}

fn appearance(values: &Shared) -> SettingPage {
    page("Appearance", IconName::Palette, "App").group(
        SettingGroup::new()
            .item(note("Mode: dark. Light mode comes later."))
            .item(SettingItem::new(
                "Chat density",
                dropdown(
                    values,
                    &[("compact", "Compact"), ("comfortable", "Comfortable")],
                    |v| &v.chat_density,
                    |v, value| v.chat_density = value,
                ),
            ))
            .item(SettingItem::new(
                "Chat width",
                dropdown(
                    values,
                    &[
                        ("narrow", "Narrow"),
                        ("standard", "Standard"),
                        ("wide", "Wide"),
                    ],
                    |v| &v.chat_width,
                    |v, value| v.chat_width = value,
                ),
            )),
    )
}

fn layout(values: &Shared) -> SettingPage {
    page("Layout & shortcuts", IconName::LayoutDashboard, "App").group(SettingGroup::new().item(
        SettingItem::new(
            "Sidebar side",
            dropdown(
                values,
                &[("left", "Left"), ("right", "Right")],
                |v| &v.sidebar_side,
                |v, value| v.sidebar_side = value,
            ),
        ),
    ))
}

fn components() -> SettingPage {
    placeholder_page("Components", IconName::Frame, "App")
}

fn updates() -> SettingPage {
    page("Updates", IconName::RefreshCw, "App").group(
        SettingGroup::new()
            .item(note(concat!(
                "Version ",
                env!("CARGO_PKG_VERSION"),
                " (development build)."
            )))
            .item(note(
                "Update checks arrive with the signed update manifest.",
            )),
    )
}
