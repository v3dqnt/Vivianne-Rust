//! Layout & shortcuts: which side the sidebar sits on, and every keyboard shortcut.

use std::collections::BTreeSet;

use gpui_kit::component::IconName;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::SharedString;

use crate::settings::values::Shared;
use crate::settings::widgets::{field_dropdown, note};

const SIDES: [(&str, &str); 2] = [("left", "Left"), ("right", "Right")];

pub fn page(values: &Shared) -> SettingPage {
    let mut page = SettingPage::new("Layout & shortcuts")
        .icon(IconName::Frame)
        .description("Where things sit, and the keys that drive them.")
        .group(SettingGroup::new().title("Layout").item(SettingItem::new(
            "Sidebar",
            field_dropdown(values, &SIDES, |v| v.sidebar_side.clone(), |v, x| v.sidebar_side = x),
        )));
    for group in shortcut_groups(values) {
        page = page.group(group);
    }
    page
}

/// One group per kind of shortcut, in the order they first appear.
fn shortcut_groups(values: &Shared) -> Vec<SettingGroup> {
    let rows: Vec<(String, String)> = values.borrow().shortcuts.iter().map(|s| (s.group.clone(), s.id.clone())).collect();
    let mut seen = BTreeSet::new();
    let order: Vec<String> = rows.iter().filter(|(g, _)| seen.insert(g.clone())).map(|(g, _)| g.clone()).collect();
    order
        .into_iter()
        .map(|group| {
            let ids = rows.iter().filter(|(g, _)| *g == group).map(|(_, id)| id.clone());
            let items: Vec<SettingItem> = ids.map(|id| shortcut_item(values, &id)).collect();
            let hint = (group == "Chat").then(|| note("Type keys as Mod+Shift+N. Mod is Ctrl on Windows and Linux, Cmd on a Mac."));
            SettingGroup::new().title(group).items(hint.into_iter().chain(items))
        })
        .collect()
}

fn shortcut_item(values: &Shared, id: &str) -> SettingItem {
    let (label, default_keys) = values
        .borrow()
        .shortcuts
        .iter()
        .find(|s| s.id == id)
        .map(|s| (s.label.clone(), s.default_keys.clone()))
        .unwrap_or_default();
    let (read, write) = (values.clone(), values.clone());
    let (read_id, write_id) = (id.to_owned(), id.to_owned());
    let field = SettingField::input(
        move |_| {
            let keys = read.borrow().shortcuts.iter().find(|s| s.id == read_id).map(|s| s.keys.clone());
            SharedString::from(keys.unwrap_or_default())
        },
        move |keys, cx| {
            if let Some(s) = write.borrow_mut().shortcuts.iter_mut().find(|s| s.id == write_id) {
                s.keys = keys.to_string();
            }
            cx.refresh_windows();
        },
    )
    .default_value(SharedString::from(default_keys));
    SettingItem::new(label, field)
}
