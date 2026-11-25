//! The system prompt as named sections in a stated order, so a part of it can be added, replaced or removed by name
//! without knowing the others. Port of the original `system-prompt` registry.
//!
//! A section has a name, an order (lower first; equal orders keep the order they were added in) and text that is either
//! fixed or asked for each time the prompt is built. A later section with the same name replaces the earlier one.
//! Empty text leaves a section out.

use std::sync::Arc;

/// The built-in sections of an agent's prompt, and where they sit. Custom sections go between by choosing an order.
pub mod order {
    /// The agent's own directive.
    pub const SYSTEM: i32 = 100;
    /// Standing context such as core memory.
    pub const CONTEXT: i32 = 500;
    /// Sections added by a caller, unless it says otherwise.
    pub const DEFAULT: i32 = 600;
    /// What the current mode asks of the agent.
    pub const MODE: i32 = 900;
}

type Dynamic = Arc<dyn Fn() -> Option<String> + Send + Sync>;

#[derive(Clone)]
enum Text {
    Fixed(String),
    Dynamic(Dynamic),
}

#[derive(Clone)]
struct Section {
    name: String,
    order: i32,
    text: Text,
}

/// Named, ordered prompt sections.
#[derive(Clone, Default)]
pub struct Sections {
    list: Vec<Section>,
}

impl std::fmt::Debug for Sections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.list.iter().map(|s| (&s.name, s.order)))
            .finish()
    }
}

impl Sections {
    /// Sets a section of fixed text, replacing one of the same name.
    pub fn set(&mut self, name: &str, order: i32, text: impl Into<String>) {
        self.put(Section {
            name: name.to_owned(),
            order,
            text: Text::Fixed(text.into()),
        });
    }

    /// Sets a section whose text is asked for each time the prompt is built (`None` leaves it out that time).
    pub fn set_dynamic(
        &mut self,
        name: &str,
        order: i32,
        text: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) {
        self.put(Section {
            name: name.to_owned(),
            order,
            text: Text::Dynamic(Arc::new(text)),
        });
    }

    /// Adds every section of `other`, replacing same-named ones: a caller's sections override the built-in ones.
    pub fn merge(&mut self, other: &Sections) {
        for section in &other.list {
            self.put(section.clone());
        }
    }

    /// Removes a section; `true` if there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|s| s.name != name);
        self.list.len() != before
    }

    /// The names in prompt order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.sorted().into_iter().map(|s| s.name.as_str()).collect()
    }

    /// The prompt: every section with text, in order, separated by a blank line. `None` when there is nothing.
    #[must_use]
    pub fn assemble(&self) -> Option<String> {
        let parts: Vec<String> = self
            .sorted()
            .into_iter()
            .filter_map(|s| match &s.text {
                Text::Fixed(text) => Some(text.clone()),
                Text::Dynamic(ask) => ask(),
            })
            .filter(|text| !text.trim().is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    fn put(&mut self, section: Section) {
        // A replacement keeps its place among equals only if the order is unchanged; otherwise it is newly added.
        match self.list.iter_mut().find(|s| s.name == section.name) {
            Some(existing) if existing.order == section.order => *existing = section,
            Some(_) => {
                self.list.retain(|s| s.name != section.name);
                self.list.push(section);
            }
            None => self.list.push(section),
        }
    }

    fn sorted(&self) -> Vec<&Section> {
        let mut sorted: Vec<&Section> = self.list.iter().collect();
        // A stable sort: equal orders stay in the order they were added.
        sorted.sort_by_key(|s| s.order);
        sorted
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn sections_come_out_in_their_order_and_equal_orders_keep_their_adding_order() {
        let mut sections = Sections::default();
        sections.set("mode", order::MODE, "mode text");
        sections.set("system", order::SYSTEM, "directive");
        sections.set("first", order::DEFAULT, "one");
        sections.set("second", order::DEFAULT, "two");
        assert_eq!(sections.names(), ["system", "first", "second", "mode"]);
        assert_eq!(
            sections.assemble().as_deref(),
            Some("directive\n\none\n\ntwo\n\nmode text")
        );
    }

    #[test]
    fn a_section_of_the_same_name_replaces_the_old_one_and_removal_is_by_name() {
        let mut sections = Sections::default();
        sections.set("rules", 10, "old");
        sections.set("other", 10, "x");
        sections.set("rules", 10, "new");
        assert_eq!(
            sections.assemble().as_deref(),
            Some("new\n\nx"),
            "an unchanged order keeps its place"
        );
        sections.set("rules", 20, "moved");
        assert_eq!(sections.assemble().as_deref(), Some("x\n\nmoved"));
        assert!(sections.remove("other") && !sections.remove("other"));
        assert_eq!(sections.assemble().as_deref(), Some("moved"));
    }

    #[test]
    fn empty_text_and_empty_prompts_leave_nothing_behind() {
        let mut sections = Sections::default();
        assert_eq!(sections.assemble(), None);
        sections.set("blank", 1, "  \n ");
        sections.set_dynamic("later", 2, || None);
        assert_eq!(sections.assemble(), None);
    }

    #[test]
    fn dynamic_text_is_asked_for_each_time_the_prompt_is_built() {
        let asked = Arc::new(AtomicUsize::new(0));
        let mut sections = Sections::default();
        let counter = asked.clone();
        sections.set_dynamic("count", 1, move || {
            Some(format!(
                "asked {}",
                counter.fetch_add(1, Ordering::SeqCst) + 1
            ))
        });
        assert_eq!(sections.assemble().as_deref(), Some("asked 1"));
        assert_eq!(sections.assemble().as_deref(), Some("asked 2"));
    }
}
