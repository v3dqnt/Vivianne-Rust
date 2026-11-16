//! The composer: a rounded card holding attachments, the text box, and a
//! toolbar (attach, thinking level, model, voice, send).

mod attachments;
mod view;

use gpui_kit::component::input::TextareaState;
use gpui_kit::{AppContext as _, Context, Entity, Window};
pub use vivianne_core::Thinking;

pub use attachments::{AttachedFile, compose_message, display_message};
pub use view::render;

/// The composer grows with its text up to this many rows.
const MAX_ROWS: usize = 8;

/// The thinking level's name in the menu and on the button.
#[must_use]
pub fn thinking_label(level: Thinking) -> &'static str {
    match level {
        Thinking::Off => "Off",
        Thinking::Low => "Low",
        Thinking::Medium => "Medium",
        Thinking::High => "High",
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
            thinking: Thinking::default(),
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
        let labels: Vec<_> = Thinking::ALL.iter().map(|&t| thinking_label(t)).collect();
        assert_eq!(labels, ["Off", "Low", "Medium", "High"]);
    }
}
