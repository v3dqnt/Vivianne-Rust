//! The text being typed. Append-only editing for now: type, paste, newline,
//! backspace. Cursor movement and selection come with the real text input.

#[derive(Debug, Default)]
pub struct Composer {
    text: String,
}

impl Composer {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn insert(&mut self, piece: &str) {
        self.text.push_str(piece);
    }

    pub fn backspace(&mut self) {
        self.text.pop();
    }

    /// Hand over the message and clear the box. `None` if there is nothing to send.
    pub fn take(&mut self) -> Option<String> {
        if self.is_blank() {
            return None;
        }
        Some(std::mem::take(&mut self.text).trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_trimmed_text_and_clears() {
        let mut c = Composer::default();
        c.insert("  hello\n");
        assert_eq!(c.take().as_deref(), Some("hello"));
        assert_eq!(c.text(), "");
    }

    #[test]
    fn blank_input_is_not_sendable() {
        let mut c = Composer::default();
        c.insert(" \n ");
        assert_eq!(c.take(), None);
        assert_eq!(c.text(), " \n ", "a refused send keeps what was typed");
    }

    #[test]
    fn backspace_removes_one_character_not_one_byte() {
        let mut c = Composer::default();
        c.insert("aé");
        c.backspace();
        assert_eq!(c.text(), "a");
    }
}
