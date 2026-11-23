//! Updates: the version you run, whether there is a newer one, and what changed.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::{Release, Shared, UpdateStatus};
use crate::settings::widgets::{btn, muted, strong};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Updates")
        .icon(IconName::Bell)
        .description("Keep Vivianne current.")
        .group(SettingGroup::new().item(status_item(values)))
        .group(
            SettingGroup::new()
                .title("What's new")
                .item(releases_item(values)),
        )
}

fn status_item(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, _window, _cx| {
        let status = values.borrow().update_status.clone();
        let checker = values.clone();
        h_flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(strong(format!("Vivianne {VERSION}")))
                    .child(muted(status_text(&status))),
            )
            .child(btn("update-check", "Check for updates", move |_, _, cx| {
                let next = next_status(&checker.borrow().update_status);
                checker.borrow_mut().update_status = next;
                cx.refresh_windows();
            }))
            .into_any_element()
    })
}

/// What the line under the version says.
fn status_text(status: &UpdateStatus) -> String {
    match status {
        UpdateStatus::Idle => "Not checked yet.".to_owned(),
        UpdateStatus::Checking => "Checking…".to_owned(),
        UpdateStatus::Available(version) => format!("Version {version} is available."),
        UpdateStatus::Latest => "You're up to date.".to_owned(),
    }
}

/// The result of pressing "Check for updates". Checking settles at once here: there is no update
/// server to ask yet.
fn next_status(_current: &UpdateStatus) -> UpdateStatus {
    UpdateStatus::Latest
}

fn releases_item(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, _window, _cx| {
        let releases = values.borrow().releases.clone();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .children(releases.iter().map(release))
            .into_any_element()
    })
}

fn release(release: &Release) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .items_baseline()
                .child(strong(release.version.clone()))
                .child(muted(release.date.clone())),
        )
        .children(release.notes.iter().map(|note| muted(format!("•  {note}"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_status_has_its_own_words() {
        assert_eq!(status_text(&UpdateStatus::Idle), "Not checked yet.");
        assert_eq!(
            status_text(&UpdateStatus::Available("1.2.0".into())),
            "Version 1.2.0 is available."
        );
        assert_eq!(
            status_text(&next_status(&UpdateStatus::Idle)),
            "You're up to date."
        );
    }
}
