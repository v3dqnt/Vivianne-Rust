//! Skills: the instruction folders the agent can pick up, and a hub to find more.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, Styled, div};

use crate::settings::values::{HubSkill, Shared, Skill};
use crate::settings::widgets::{keyed_input, mono, muted, pill, strong};

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Skills")
        .icon(IconName::Star)
        .description("Folders of instructions the agent can load when a task calls for them.")
        .group(installed_group(values))
        .group(hub_group(values))
}

fn installed_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().title("Installed").item(SettingItem::render(move |_options, window, cx| {
        let search = keyed_input("skill-search", &values, |v| v.skill_search.clone(), |v, x| v.skill_search = x, "Filter skills", window, cx);
        let (query, skills) = {
            let v = values.borrow();
            (v.skill_search.to_lowercase(), v.skills.clone())
        };
        let rows = skills.into_iter().filter(|s| matches_query(s, &query)).map(|s| skill_row(&s));
        div().flex().flex_col().gap_3().child(search).children(rows).into_any_element()
    }))
}

/// Whether a skill's name or description holds every word of the filter.
fn matches_query(skill: &Skill, query: &str) -> bool {
    let haystack = format!("{} {}", skill.name, skill.desc).to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

fn skill_row(skill: &Skill) -> impl IntoElement + use<> {
    div().flex().flex_col().gap_1().child(strong(skill.name.clone())).child(muted(skill.desc.clone())).child(mono(skill.path.clone()))
}

fn hub_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Find more")
        .description("Skills shared by others. Installing copies a folder into your skills.")
        .item(SettingItem::render(move |_options, window, cx| {
            let search = keyed_input("hub-query", &values, |v| v.hub_query.clone(), |v, x| v.hub_query = x, "Search the hub", window, cx);
            let (query, hub) = {
                let v = values.borrow();
                (v.hub_query.to_lowercase(), v.hub.clone())
            };
            let rows = hub
                .into_iter()
                .enumerate()
                .filter(|(_, s)| hub_matches(s, &query))
                .map(|(index, s)| hub_row(&values, index, &s));
            div().flex().flex_col().gap_3().child(search).children(rows).into_any_element()
        }))
}

fn hub_matches(skill: &HubSkill, query: &str) -> bool {
    let haystack = format!("{} {}", skill.name, skill.desc).to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

fn hub_row(values: &Shared, index: usize, skill: &HubSkill) -> impl IntoElement + use<> {
    let toggle = values.clone();
    let label = if skill.installed { "Remove" } else { "Install" };
    let button = Button::new(("hub-skill", index)).label(label).small().on_click(move |_, _, cx| {
        let mut v = toggle.borrow_mut();
        let Some(entry) = v.hub.get_mut(index) else { return };
        entry.installed = !entry.installed;
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
                .child(h_flex().gap_2().items_center().child(strong(skill.name.clone())).child(pill(SharedString::from(format!("{} files", skill.files)))))
                .child(muted(skill.desc.clone())),
        )
        .child(button)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, desc: &str) -> Skill {
        Skill { name: name.into(), desc: desc.into(), path: String::new() }
    }

    #[test]
    fn a_filter_needs_every_word_in_the_name_or_description() {
        let s = skill("pdf-forms", "Fill in PDF forms");
        assert!(matches_query(&s, ""));
        assert!(matches_query(&s, "pdf fill"));
        assert!(!matches_query(&s, "pdf spreadsheet"));
    }
}
