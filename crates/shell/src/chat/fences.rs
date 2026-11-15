//! Split reply text into prose and fenced code blocks. A fence still open at
//! the end (the reply is mid-stream) counts as code, so blocks do not flicker.

#[derive(Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Prose(&'a str),
    Code { lang: &'a str, body: &'a str },
}

const FENCE: &str = "```";

#[must_use]
pub fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(FENCE) {
        push_prose(&mut out, &rest[..open]);
        let after = &rest[open + FENCE.len()..];
        let (lang, body_start) = match after.find('\n') {
            Some(newline) => (after[..newline].trim(), newline + 1),
            None => (after.trim(), after.len()),
        };
        let body_and_rest = &after[body_start..];
        let (body, next) = match body_and_rest.find(FENCE) {
            Some(close) => (
                &body_and_rest[..close],
                &body_and_rest[close + FENCE.len()..],
            ),
            None => (body_and_rest, ""),
        };
        out.push(Segment::Code {
            lang,
            body: body.trim_end_matches('\n'),
        });
        rest = next;
    }
    push_prose(&mut out, rest);
    out
}

fn push_prose<'a>(out: &mut Vec<Segment<'a>>, text: &'a str) {
    let text = text.trim_matches('\n');
    if !text.is_empty() {
        out.push(Segment::Prose(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_prose_segment() {
        assert_eq!(segments("hello"), vec![Segment::Prose("hello")]);
    }

    #[test]
    fn code_between_prose_is_split_out_with_its_language() {
        let got = segments("before\n```rust\nfn x() {}\n```\nafter");
        assert_eq!(
            got,
            vec![
                Segment::Prose("before"),
                Segment::Code {
                    lang: "rust",
                    body: "fn x() {}"
                },
                Segment::Prose("after"),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_is_code_while_streaming() {
        let got = segments("run:\n```sh\ncargo te");
        assert_eq!(
            got[1],
            Segment::Code {
                lang: "sh",
                body: "cargo te"
            }
        );
    }

    #[test]
    fn empty_text_has_no_segments() {
        assert!(segments("").is_empty());
    }
}
