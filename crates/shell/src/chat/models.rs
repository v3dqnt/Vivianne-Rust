//! The model picker's data: every model the saved keys reach, as rows a searchable list can show.

use gpui_kit::component::combobox::ComboboxState;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, SharedString, Window, div};
use vivianne_ai::ai::model_discovery::DiscoveredModel;

use crate::theme::{self, color};

/// One model of one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub provider: String,
    pub id: String,
    pub name: String,
    /// `provider/id`, what the picker hands back.
    value: String,
}

impl ModelChoice {
    #[must_use]
    pub fn new(provider: &str, id: &str, name: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            value: Self::value_of(provider, id),
        }
    }

    /// The picker value for a provider's model.
    #[must_use]
    pub fn value_of(provider: &str, id: &str) -> String {
        format!("{provider}/{id}")
    }
}

impl SearchableListItem for ModelChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone().into()
    }

    fn value(&self) -> &String {
        &self.value
    }

    /// Search by name, id or provider, so "nvidia kimi" and "gpt-5" both find what they should.
    fn matches(&self, query: &str) -> bool {
        let haystack = format!("{} {} {}", self.provider, self.id, self.name).to_lowercase();
        query.to_lowercase().split_whitespace().all(|word| haystack.contains(word))
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        row(&self.name, &self.provider)
    }
}

fn row(name: &str, provider: &str) -> AnyElement {
    div()
        .flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().truncate().child(name.to_owned()))
        .child(div().text_xs().text_color(color(theme::MUTED)).child(provider.to_owned()))
        .into_any_element()
}

/// The picker's list state.
pub type Picker = Entity<ComboboxState<SearchableVec<ModelChoice>>>;

/// Chat models only (no image or audio generators), grouped by provider, A to Z.
#[must_use]
pub fn choices(models: &[DiscoveredModel]) -> Vec<ModelChoice> {
    let mut out: Vec<ModelChoice> = models
        .iter()
        .filter(|m| !m.is_image_model && !m.is_audio_model)
        .map(|m| ModelChoice::new(&m.provider, &m.id, &m.name))
        .collect();
    out.sort_by(|a, b| (&a.provider, a.name.to_lowercase()).cmp(&(&b.provider, b.name.to_lowercase())));
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered(provider: &str, id: &str, image: bool) -> DiscoveredModel {
        DiscoveredModel {
            provider: provider.into(),
            id: id.into(),
            name: id.into(),
            is_image_model: image,
            ..DiscoveredModel::default()
        }
    }

    #[test]
    fn image_models_are_left_out_and_the_rest_sorted_by_provider() {
        let found = [
            discovered("openai", "gpt-5", false),
            discovered("google", "imagen", true),
            discovered("google", "gemini", false),
        ];
        let names: Vec<_> = choices(&found).iter().map(|c| c.value.clone()).collect();
        assert_eq!(names, ["google/gemini", "openai/gpt-5"]);
    }

    #[test]
    fn search_finds_a_model_by_any_part_of_its_name() {
        let model = ModelChoice::new("nvidia", "moonshotai/kimi-k2.6", "Kimi K2.6");
        assert!(model.matches("kimi"));
        assert!(model.matches("nvidia kimi"));
        assert!(model.matches("MOONSHOT"));
        assert!(!model.matches("gpt"));
    }

    #[test]
    fn the_picker_value_is_provider_slash_id() {
        assert_eq!(ModelChoice::value_of("openai", "gpt-5-nano"), "openai/gpt-5-nano");
    }
}
