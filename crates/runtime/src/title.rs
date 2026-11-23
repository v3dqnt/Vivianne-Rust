//! A thread's title: a short summary of what was asked, not a copy of it.
//!
//! Until a model writes one, the first message cleaned into a sentence is the title ([`fallback_title`]). A title
//! the user typed is never replaced (see [`decide`]).

use vivianne_core::{Message, Role};

use crate::compact::OPEN;

/// Who named a thread.
pub const USER: &str = "user";
pub const AUTO: &str = "auto";

/// Longest title, in characters, that a summary or a cleaned sentence may be.
const MAX_CHARS: usize = 48;

/// What was asked, cleaned into something that reads as a title: the app's own markers, code fences, links and
/// markdown gone, the first sentence only, cut at a word, never in the middle of one.
#[must_use]
pub fn fallback_title(text: &str) -> String {
    let mut s = text.to_owned();
    // <tags>, ``` fences with their contents, inline code marks, urls, markdown marks.
    while let (Some(a), Some(b)) = (s.find('<'), s.find('>')) {
        if a < b {
            s.replace_range(a..=b, " ");
        } else {
            break;
        }
    }
    while let Some(a) = s.find("```") {
        match s[a + 3..].find("```") {
            Some(b) => s.replace_range(a..a + 3 + b + 3, " "),
            None => {
                s.truncate(a);
                break;
            }
        }
    }
    let words: Vec<&str> = s
        .split_whitespace()
        .filter(|w| !w.starts_with("http://") && !w.starts_with("https://"))
        .collect();
    let mut s = words.join(" ");
    s.retain(|c| !matches!(c, '*' | '_' | '`' | '#' | '>'));
    let s = s.trim();
    // The first sentence: up to a full stop, question or exclamation mark that ends a word.
    let mut end = s.len();
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        if matches!(c, '.' | '?' | '!') && chars.get(i + 1).is_none_or(|&(_, n)| n == ' ') && at > 8
        {
            end = at + c.len_utf8();
            break;
        }
    }
    let sentence = s[..end].trim().trim_end_matches(['.', ',', ':', ';']);
    if sentence.is_empty() {
        return "New chat".to_owned();
    }
    let cut = cut_at_word(sentence, MAX_CHARS);
    let mut chars = cut.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn cut_at_word(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max).collect();
    let cut = match head.rfind(' ') {
        Some(i) if i >= max / 2 => &head[..i],
        _ => head.as_str(),
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', '-', ' ']))
}

/// The system and user prompts that ask a model for a title.
#[must_use]
pub fn title_prompt(first_ask: &str) -> (String, String) {
    let system = "Write a title for a chat that opens with the message below. Three to six words. Say what the person wants, as a noun phrase or a short imperative (for example: Fix login timeout, Plot cat weights, Review uncommitted changes). Use the same language as the message. No quotes, no trailing full stop, no markdown. Answer with the title only.".to_owned();
    (system, first_ask.chars().take(1500).collect())
}

/// A model's answer as a title, or `None` when it isn't one.
#[must_use]
pub fn parse_title(answer: &str) -> Option<String> {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut t = line.trim();
    for prefix in ["Title:", "title:", "TITLE:"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim();
        }
    }
    // Quotes and a full stop can wrap each other (`"Title".` or `"Title."`), so trim until neither is left.
    let t = t.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | '*' | '“' | '”' | '‘' | '’' | '#' | '.'
        ) || c.is_whitespace()
    });
    if t.is_empty() || t.chars().count() > 60 || t.split_whitespace().count() > 12 {
        return None;
    }
    Some(t.to_owned())
}

/// The title a save writes, and its source: one the user typed stays, one already made stays, and otherwise the
/// first message is cleaned into one.
#[must_use]
pub fn decide(existing: Option<(&str, &str)>, first_ask: Option<&str>) -> (String, &'static str) {
    match existing {
        Some((name, USER)) if !name.trim().is_empty() => (name.to_owned(), USER),
        Some((name, AUTO)) if !name.trim().is_empty() && name != "New chat" => {
            (name.to_owned(), AUTO)
        }
        _ => (
            first_ask.map_or_else(|| "New chat".to_owned(), fallback_title),
            AUTO,
        ),
    }
}

/// The first thing a person asked: the first user message that is not a condensed-history checkpoint.
#[must_use]
pub fn first_ask(messages: &[Message]) -> Option<&str> {
    messages
        .iter()
        .find(|m| {
            m.role == Role::User
                && !m.text.trim().is_empty()
                && !m.text.trim_start().starts_with(OPEN)
        })
        .map(|m| m.text.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_first_sentence_cleaned_and_cut_at_a_word() {
        assert_eq!(
            fallback_title("fix the login timeout. It happens on mobile"),
            "Fix the login timeout"
        );
        assert_eq!(
            fallback_title("```rust\nfn main() {}\n``` why does this panic?"),
            "Why does this panic?"
        );
        assert_eq!(
            fallback_title("see https://example.com/x for **context**"),
            "See for context"
        );
        let long = fallback_title(
            "please refactor the authentication module so that sessions are stored in the database",
        );
        assert!(long.chars().count() <= 49 && long.ends_with('…'), "{long}");
        assert_eq!(fallback_title("   "), "New chat");
    }

    #[test]
    fn a_models_answer_is_a_title_only_if_it_looks_like_one() {
        assert_eq!(
            parse_title("Title: \"Fix login timeout\"."),
            Some("Fix login timeout".to_owned())
        );
        assert_eq!(
            parse_title("\n\n**Plot cat weights**\nmore"),
            Some("Plot cat weights".to_owned())
        );
        assert!(parse_title(&"word ".repeat(20)).is_none());
        assert!(parse_title("  ").is_none());
    }

    #[test]
    fn what_the_user_typed_is_never_replaced_and_a_made_title_stays() {
        assert_eq!(
            decide(Some(("My name", USER)), Some("something else")),
            ("My name".to_owned(), USER)
        );
        assert_eq!(
            decide(Some(("Fix login", AUTO)), Some("other")),
            ("Fix login".to_owned(), AUTO)
        );
        assert_eq!(
            decide(None, Some("fix the build please")),
            ("Fix the build please".to_owned(), AUTO)
        );
        assert_eq!(
            decide(Some(("New chat", AUTO)), Some("hello there friend")).0,
            "Hello there friend"
        );
    }

    #[test]
    fn the_first_ask_skips_a_checkpoint() {
        let history = vec![
            Message::new(Role::User, format!("{OPEN}\nsummary")),
            Message::new(Role::User, "the real ask"),
        ];
        assert_eq!(first_ask(&history), Some("the real ask"));
    }
}
