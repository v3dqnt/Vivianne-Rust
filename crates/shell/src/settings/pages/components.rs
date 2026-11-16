//! Components: the optional pieces Vivianne can install, and what each is for.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::{ComponentEntry, Shared};
use crate::settings::widgets::{muted, pill, strong};

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Components")
        .icon(IconName::Inbox)
        .description("Optional pieces that add abilities. Install only what you use.")
        .group(SettingGroup::new().item(list(values)))
}

fn list(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, _window, _cx| {
        let entries = values.borrow().components.clone();
        let rows = entries.into_iter().enumerate().map(|(index, entry)| row(&values, index, &entry));
        div().flex().flex_col().gap_4().children(rows).into_any_element()
    })
}

fn row(values: &Shared, index: usize, entry: &ComponentEntry) -> impl IntoElement + use<> {
    let toggle = values.clone();
    let label = if entry.installed { "Remove" } else { "Install" };
    let button = Button::new(("component", index)).label(label).small().on_click(move |_, _, cx| {
        toggle_installed(&mut toggle.borrow_mut().components[index]);
        cx.refresh_windows();
    });
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(h_flex().gap_2().items_center().child(strong(entry.name.clone())).child(pill(entry.size.clone())))
                .child(muted(entry.desc.clone()))
                .children(entry.note.clone().map(muted)),
        )
        .child(h_flex().gap_2().items_center().children(entry.installed.then(|| pill("Installed"))).child(button))
}

/// Installing and removing flip the flag; a real install would run, report progress, then flip it.
fn toggle_installed(entry: &mut ComponentEntry) {
    entry.installed = !entry.installed;
    entry.running = false;
    entry.progress = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_then_remove_round_trips() {
        let mut entry = ComponentEntry {
            id: "x".into(),
            name: "X".into(),
            size: "1 MB".into(),
            desc: String::new(),
            installed: false,
            running: true,
            progress: Some(0.4),
            note: None,
        };
        toggle_installed(&mut entry);
        assert!(entry.installed && !entry.running && entry.progress.is_none());
        toggle_installed(&mut entry);
        assert!(!entry.installed);
    }
}
