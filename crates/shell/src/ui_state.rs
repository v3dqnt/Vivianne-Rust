//! What the window remembers between launches: the model last used, and the side panel's open tab. A small file,
//! `~/.viv/agent/viv-ui-state.json`; a missing or unreadable one is the same as an empty one.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// `(provider, model)` of the last model picked for a thread.
    #[serde(default)]
    pub model: Option<(String, String)>,
    /// The side panel's tab by name (`overview`, `files`, `preview`, `terminal`).
    #[serde(default)]
    pub rail_tab: Option<String>,
    /// The Overview's sections that are minimised, by name (`context`, `version-control`).
    #[serde(default)]
    pub rail_collapsed: Vec<String>,
}

fn file() -> std::path::PathBuf {
    vivianne_ai::paths::viv_home_path(&["agent", "viv-ui-state.json"])
}

fn read_at(path: &std::path::Path) -> State {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[must_use]
pub fn load() -> State {
    read_at(&file())
}

fn update_at(path: &std::path::Path, change: impl FnOnce(&mut State)) {
    let mut state = read_at(path);
    change(&mut state);
    // Remembering is a nicety: a file that cannot be written costs only the memory.
    let _ = vivianne_ai::paths::write_json_atomic(path, &state);
}

pub fn save_model(provider: &str, model: &str) {
    update_at(&file(), |s| {
        s.model = Some((provider.to_owned(), model.to_owned()))
    });
}

pub fn save_rail_tab(tab: &str) {
    update_at(&file(), |s| s.rail_tab = Some(tab.to_owned()));
}

pub fn save_rail_collapsed(sections: Vec<String>) {
    update_at(&file(), |s| s.rail_collapsed = sections);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_thing_remembered_leaves_the_other_alone_and_a_broken_file_is_empty() {
        let path = std::env::temp_dir().join(format!("viv-ui-state-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        update_at(&path, |s| s.model = Some(("p".into(), "m".into())));
        update_at(&path, |s| s.rail_tab = Some("files".into()));
        assert_eq!(
            read_at(&path),
            State {
                model: Some(("p".into(), "m".into())),
                rail_tab: Some("files".into())
            }
        );
        std::fs::write(&path, "not json").expect("writes");
        assert_eq!(read_at(&path), State::default());
        let _ = std::fs::remove_file(path);
    }
}
