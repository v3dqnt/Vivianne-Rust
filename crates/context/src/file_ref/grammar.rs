//! The `@file` token grammar: what counts as an `@` being typed, and how a chosen path is written back into the prompt as
//! ordinary text. Port of the original `grammar.ts`.

/// The `@` token that ends at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAtToken {
    /// The whole token, replaced when a completion is accepted.
    pub prefix: String,
    /// The path typed after `@` or `@"`.
    pub query: String,
    /// Whether the quote was opened.
    pub quoted: bool,
}

fn starts_token(before: &str, at: usize) -> bool {
    at == 0
        || before[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
}

/// The `@path` or `@"path with spaces` token at the cursor. An `@` inside another token, such as an email address, does not
/// start one. `cursor` is a byte offset into `line` on a character boundary.
#[must_use]
pub fn active_at_token(line: &str, cursor: usize) -> Option<ActiveAtToken> {
    let before = line.get(..cursor)?;
    // `@"` ... with no closing quote yet, after the start of a token.
    let mut from = before.len();
    while let Some(at) = before[..from].rfind("@\"") {
        if starts_token(before, at) && !before[at + 2..].contains('"') {
            return Some(ActiveAtToken {
                prefix: before[at..].to_owned(),
                query: before[at + 2..].to_owned(),
                quoted: true,
            });
        }
        from = at;
    }
    let start = before.rfind(char::is_whitespace).map_or(0, |i| {
        i + before[i..].chars().next().map_or(1, char::len_utf8)
    });
    let token = &before[start..];
    token.strip_prefix('@').map(|query| ActiveAtToken {
        prefix: token.to_owned(),
        query: query.to_owned(),
        quoted: false,
    })
}

/// Whether a candidate is a file or a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
}

/// One path-only completion candidate inside the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    pub kind: Kind,
}

/// A chosen path as prompt text. A path with whitespace uses the quoted `@"path"` form; a quoted folder keeps the quote
/// open after its trailing slash so completion can go one level deeper. `None` for a path the grammar cannot hold safely.
#[must_use]
pub fn format_file_mention(candidate: &Candidate, preserve_quote: bool) -> Option<String> {
    let path = if candidate.kind == Kind::Directory {
        format!("{}/", candidate.path)
    } else {
        candidate.path.clone()
    };
    if path.chars().any(|c| c == '"' || c.is_control()) {
        return None;
    }
    let quoted = preserve_quote || path.chars().any(char::is_whitespace);
    Some(match (quoted, candidate.kind) {
        (false, _) => format!("@{path}"),
        (true, Kind::Directory) => format!("@\"{path}"),
        (true, Kind::File) => format!("@\"{path}\""),
    })
}

/// What the model is told about `@` paths.
pub const FILE_REFERENCE_PROMPT: &str = "Tokens prefixed with @ are paths the user explicitly referenced. Relative paths resolve from the workspace root; absolute paths identify files or directories on the host. A trailing slash marks a directory: list it when its contents matter. Anything else is a file: use the read tool when its contents are needed, and do not claim to have inspected it before reading. @\"...\" quotes a path containing spaces.";
