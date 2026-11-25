//! Quoting part of a message into the next one. Port of the idea behind the original `message-quotes`: a quote is ordinary
//! Markdown, one `> ` before every line, with no metadata or index kept beside it, so the model sees plain text and nothing
//! about history changes. This module turns selected text into that form and reads it back, so a screen can show a quote
//! as a quote.

/// `text` as a blockquote, ready to put in front of what the person writes. An empty line inside it stays part of the
/// quote (`>`), and a quote of nothing is nothing.
#[must_use]
pub fn quote(text: &str) -> String {
    let text = text.trim_matches(['\r', '\n']);
    if text.trim().is_empty() {
        return String::new();
    }
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let line = line.trim_end();
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect();
    format!("{}\n\n", lines.join("\n"))
}

/// A message split into the quotes it opens with and the person's own words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Split {
    /// Each quote, without its `> ` prefixes. Quotes are separated by a blank line or by the end of the run of `>` lines.
    pub quotes: Vec<String>,
    /// What follows the quotes.
    pub body: String,
}

fn unquoted(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Reads the quotes a message begins with. Only leading blockquote lines count: a `>` later in the text is the person's
/// own Markdown.
#[must_use]
pub fn split(message: &str) -> Split {
    let mut quotes: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut rest = message;
    loop {
        let (line, after) = rest.split_once('\n').map_or((rest, ""), |(l, a)| (l, a));
        match unquoted(line) {
            Some(text) => current.push(text),
            None if line.trim().is_empty() && !current.is_empty() => {
                quotes.push(current.join("\n"));
                current.clear();
            }
            None if line.trim().is_empty() && quotes.is_empty() => {}
            None => break,
        }
        if after.is_empty() && !rest.contains('\n') {
            rest = "";
            break;
        }
        rest = after;
    }
    if !current.is_empty() {
        quotes.push(current.join("\n"));
    }
    Split {
        quotes,
        body: rest.trim_start_matches('\n').to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_gets_a_prefix_and_blank_lines_stay_inside_the_quote() {
        assert_eq!(quote("one\n\ntwo\n"), "> one\n>\n> two\n\n");
        assert_eq!(quote("  \n"), "");
        assert_eq!(quote("trailing   "), "> trailing\n\n");
    }

    #[test]
    fn a_quote_reads_back_as_what_was_quoted_and_the_persons_words_after_it() {
        let message = format!("{}what does this mean?", quote("first\n\nsecond"));
        assert_eq!(
            split(&message),
            Split {
                quotes: vec!["first\n\nsecond".to_owned()],
                body: "what does this mean?".to_owned()
            }
        );
    }

    #[test]
    fn separate_quotes_stay_separate_and_a_later_arrow_is_not_a_quote() {
        let message = format!("{}{}answer with a > b", quote("one"), quote("two"));
        let got = split(&message);
        assert_eq!(
            (got.quotes.as_slice(), got.body.as_str()),
            (
                ["one".to_owned(), "two".to_owned()].as_slice(),
                "answer with a > b"
            )
        );
        assert_eq!(
            split("plain\n> not a leading quote"),
            Split {
                quotes: vec![],
                body: "plain\n> not a leading quote".to_owned()
            }
        );
        assert_eq!(
            split("> only a quote"),
            Split {
                quotes: vec!["only a quote".to_owned()],
                body: String::new()
            }
        );
        assert_eq!(split(""), Split::default());
    }
}
