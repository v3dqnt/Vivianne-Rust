//! The model picker's data: every model the saved keys reach, as rows a searchable list can show.

use gpui_kit::component::combobox::ComboboxState;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, SharedString, Window, div};
use vivianne_ai::ai::model_discovery::DiscoveredModel;

use crate::theme::{self, color};

/// The `provider` of a coding agent in the picker (the Agent Client Protocol layer's own name for it).
pub const AGENT_PROVIDER: &str = vivianne_providers::acp::PROVIDER;

/// One model of one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub provider: String,
    pub id: String,
    pub name: String,
    /// Tokens the model can hold at once, when its provider says (many don't).
    pub context_window: Option<u32>,
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
            context_window: None,
            value: Self::value_of(provider, id),
        }
    }

    /// A coding agent, shown in the same list as the models.
    #[must_use]
    pub fn agent(id: &str, name: &str) -> Self {
        Self::new(AGENT_PROVIDER, id, name)
    }

    #[must_use]
    pub fn is_agent(&self) -> bool {
        self.provider == AGENT_PROVIDER
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
        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        row(
            &self.name,
            if self.is_agent() {
                "Coding agent"
            } else {
                &self.provider
            },
        )
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
        .child(
            div()
                .text_xs()
                .text_color(color(theme::MUTED))
                .child(provider.to_owned()),
        )
        .into_any_element()
}

/// The picker's list state.
pub type Picker = Entity<ComboboxState<SearchableVec<ModelChoice>>>;

/// Chat models only (no image or audio generators), grouped by provider, A to Z, then the coding agents
/// (id and name each) that can run on this computer.
#[must_use]
pub fn choices(models: &[DiscoveredModel], agents: &[(String, String)]) -> Vec<ModelChoice> {
    let mut out: Vec<ModelChoice> = models
        .iter()
        .filter(|m| !m.is_image_model && !m.is_audio_model)
        .map(|m| ModelChoice {
            context_window: m.context_window,
            ..ModelChoice::new(&m.provider, &m.id, &m.name)
        })
        .collect();
    out.sort_by(|a, b| {
        (&a.provider, a.name.to_lowercase()).cmp(&(&b.provider, b.name.to_lowercase()))
    });
    out.dedup();
    out.extend(agents.iter().map(|(id, name)| ModelChoice::agent(id, name)));
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
        let names: Vec<_> = choices(&found, &[])
            .iter()
            .map(|c| c.value.clone())
            .collect();
        assert_eq!(names, ["google/gemini", "openai/gpt-5"]);
    }

    #[test]
    fn coding_agents_follow_the_models_and_are_told_apart() {
        let found = [discovered("openai", "gpt-5", false)];
        let list = choices(
            &found,
            &[("claude-acp".to_owned(), "Claude Code".to_owned())],
        );
        assert_eq!(list.len(), 2);
        assert!(list[1].is_agent() && !list[0].is_agent());
        assert_eq!(list[1].value, "harness/claude-acp");
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
        assert_eq!(
            ModelChoice::value_of("openai", "gpt-5-nano"),
            "openai/gpt-5-nano"
        );
    }
}
