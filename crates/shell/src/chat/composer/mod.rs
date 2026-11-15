//! The composer: a rounded card holding attachments, the text box, and a
//! toolbar (attach, thinking level, model, voice, send).

mod attachments;
mod view;

use gpui_kit::component::input::TextareaState;
use gpui_kit::{AppContext as _, Context, Entity, Window};

pub use attachments::{AttachedFile, compose_message, display_message};
pub use view::render;

/// The composer grows with its text up to this many rows.
const MAX_ROWS: usize = 8;

/// How much the model should think before answering. Chosen here, but not sent
/// to the provider yet: that arrives with provider reasoning settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    Off,
    Low,
    Medium,
    High,
}

impl Thinking {
    pub const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

pub struct Composer {
    pub input: Entity<TextareaState>,
    pub attachments: Vec<AttachedFile>,
    pub thinking: Thinking,
}

impl Composer {
    pub fn new<T: 'static>(hint: String, window: &mut Window, cx: &mut Context<T>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .submit_on_enter(true)
                .placeholder(hint)
        });
        Self {
            input,
            attachments: Vec::new(),
            thinking: Thinking::Medium,
        }
    }

    /// Add files, skipping folders, unreadable paths and ones already attached.
    pub fn attach(&mut self, paths: &[std::path::PathBuf]) {
        for path in paths {
            if let Some(file) = AttachedFile::from_path(path)
                && !self.attachments.iter().any(|a| a.path == file.path)
            {
                self.attachments.push(file);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_levels_are_ordered_and_named() {
        let labels: Vec<_> = Thinking::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(labels, ["Off", "Low", "Medium", "High"]);
    }
}
