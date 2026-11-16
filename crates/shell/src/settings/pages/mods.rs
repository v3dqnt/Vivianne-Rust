//! Mods: short instructions that change how the agent behaves, and the tools it may use.

use std::collections::BTreeSet;

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;

use crate::settings::values::Shared;
use crate::settings::widgets::{field_input, muted, note, strong};

pub fn page(values: &Shared) -> SettingPage {
    let mut page = SettingPage::new("Mods & tools")
        .icon(IconName::Asterisk)
        .description("Instructions that shape the agent, and the tools it can use.")
        .group(mods_group(values))
        .group(guides_group(values));
    for group in tool_groups(values) {
        page = page.group(group);
    }
    page.group(used_group(values))
}

fn switch(read: Shared, write: Shared, get: impl Fn(&crate::settings::values::Values) -> bool + 'static, set: impl Fn(&mut crate::settings::values::Values, bool) + 'static) -> SettingField<bool> {
    SettingField::switch(
        move |_| get(&read.borrow()),
        move |on, cx| {
            set(&mut write.borrow_mut(), on);
            cx.refresh_windows();
        },
    )
}

fn mods_group(values: &Shared) -> SettingGroup {
    let count = values.borrow().mods.len();
    let items = (0..count).map(|index| {
        let (name, desc) = {
            let v = values.borrow();
            (v.mods[index].name.clone(), v.mods[index].desc.clone())
        };
        let field = switch(values.clone(), values.clone(), move |v| v.mods[index].enabled, move |v, on| v.mods[index].enabled = on);
        SettingItem::new(name, field).description(desc)
    });
    SettingGroup::new().title("Mods").items(items)
}

fn guides_group(values: &Shared) -> SettingGroup {
    SettingGroup::new().item(
        SettingItem::new("Guides repository", field_input(values, |v| v.guides_repo.clone(), |v, x| v.guides_repo = x))
            .description("Where shared mods and guides are fetched from."),
    )
}

/// One group per kind of tool, in the order they first appear.
fn tool_groups(values: &Shared) -> Vec<SettingGroup> {
    let tools: Vec<(String, String)> = values.borrow().tools.iter().map(|t| (t.group.clone(), t.id.clone())).collect();
    let mut seen = BTreeSet::new();
    let order: Vec<String> = tools.iter().filter(|(g, _)| seen.insert(g.clone())).map(|(g, _)| g.clone()).collect();
    order
        .into_iter()
        .map(|group| {
            let items = tools.iter().filter(|(g, _)| *g == group).map(|(_, id)| tool_item(values, id));
            SettingGroup::new().title(format!("Tools: {group}")).items(items.collect::<Vec<_>>())
        })
        .collect()
}

fn tool_item(values: &Shared, id: &str) -> SettingItem {
    let label = values.borrow().tools.iter().find(|t| t.id == id).map(|t| t.label.clone()).unwrap_or_default();
    let (get_id, set_id) = (id.to_owned(), id.to_owned());
    let field = switch(
        values.clone(),
        values.clone(),
        move |v| v.tools.iter().any(|t| t.id == get_id && t.enabled),
        move |v, on| {
            if let Some(t) = v.tools.iter_mut().find(|t| t.id == set_id) {
                t.enabled = on;
            }
        },
    );
    SettingItem::new(label, field)
}

fn used_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Made by Vivianne")
        .item(note("Tools the agent wrote for itself while working, with how often each ran."))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let tools = values.borrow().custom_tools.clone();
            if tools.is_empty() {
                return muted("None yet.").into_any_element();
            }
            let rows = tools.into_iter().map(|t| {
                h_flex().justify_between().child(strong(t.name)).child(muted(format!("{} uses, last {}", t.uses, t.last)))
            });
            gpui_kit::div().flex().flex_col().gap_1().children(rows).into_any_element()
        }))
}
