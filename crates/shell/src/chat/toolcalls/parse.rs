//! Reads the text the original tools return into shapes the views can draw.
//! Each parser follows the format its tool writes (see the tool's source) and
//! returns `None` when the text is not in that shape, so the view falls back to
//! plain output. No GPUI here, so it is unit tested.

/// `read`: `"{n:>6}\t{line}"` rows, then `[N lines total]` or a truncation note.
#[derive(Debug, PartialEq, Eq)]
pub struct ReadView {
    pub rows: Vec<(usize, String)>,
    pub notes: Vec<String>,
}

#[must_use]
pub fn read(output: &str) -> Option<ReadView> {
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for line in output.lines() {
        if let Some((number, text)) = line.split_once('\t')
            && let Ok(n) = number.trim().parse::<usize>()
        {
            rows.push((n, text.to_owned()));
        } else if !line.trim().is_empty() {
            notes.push(line.trim().to_owned());
        }
    }
    (!rows.is_empty()).then_some(ReadView { rows, notes })
}

/// `grep`: `path:line:text` rows grouped by file, then notes (`... N more matches`, a PARTIAL warning).
#[derive(Debug, PartialEq, Eq)]
pub struct GrepView {
    pub files: Vec<(String, Vec<(usize, String)>)>,
    pub notes: Vec<String>,
}

/// Split `path:12:text`. The path may itself hold colons (`C:/x/y.rs:12:text`).
fn split_hit(line: &str) -> Option<(&str, usize, &str)> {
    for (i, _) in line.match_indices(':') {
        let rest = &line[i + 1..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if i > 0 && digits > 0 && rest[digits..].starts_with(':') {
            let number = rest[..digits].parse().ok()?;
            return Some((&line[..i], number, &rest[digits + 1..]));
        }
    }
    None
}

#[must_use]
pub fn grep(output: &str) -> Option<GrepView> {
    let mut files: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    let mut notes = Vec::new();
    for line in output.lines() {
        match split_hit(line) {
            Some((path, number, text)) => match files.last_mut() {
                Some((last, hits)) if last == path => hits.push((number, text.to_owned())),
                _ => files.push((path.to_owned(), vec![(number, text.to_owned())])),
            },
            None if !line.trim().is_empty() => notes.push(line.trim().to_owned()),
            None => {}
        }
    }
    (!files.is_empty()).then_some(GrepView { files, notes })
}

/// `glob`: one path per line, then an optional `(Results truncated ...)` note.
#[derive(Debug, PartialEq, Eq)]
pub struct GlobView {
    pub paths: Vec<String>,
    pub note: Option<String>,
}

#[must_use]
pub fn glob(output: &str) -> Option<GlobView> {
    if output.trim() == "No files found" {
        return Some(GlobView {
            paths: Vec::new(),
            note: None,
        });
    }
    let mut paths = Vec::new();
    let mut note = None;
    for line in output.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('(') {
            note = Some(line.trim_matches(|c| c == '(' || c == ')').to_owned());
        } else {
            paths.push(line.trim().to_owned());
        }
    }
    (!paths.is_empty()).then_some(GlobView { paths, note })
}

/// `ls`: a `Listing ...` header, then rows indented two spaces per level, folders marked with a folder emoji.
#[derive(Debug, PartialEq, Eq)]
pub struct LsView {
    pub rows: Vec<LsRow>,
    pub notes: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LsRow {
    pub depth: usize,
    pub name: String,
    pub is_dir: bool,
}

#[must_use]
pub fn ls(output: &str) -> LsView {
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for line in output.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let trimmed = line.trim_start();
        let depth = (line.len() - trimmed.len()) / 2;
        let marked = ["📁", "📄"].iter().find_map(|m| trimmed.strip_prefix(m));
        match marked {
            Some(rest) => {
                let name = rest.trim();
                rows.push(LsRow {
                    depth,
                    is_dir: trimmed.starts_with("📁"),
                    name: name.trim_end_matches('/').to_owned(),
                });
            }
            None => notes.push(trimmed.to_owned()),
        }
    }
    LsView { rows, notes }
}

/// `code_search`: a `N results for: ...` header, then `path:start-end  (score S)` blocks with indented previews.
#[derive(Debug, PartialEq)]
pub struct SearchView {
    pub header: String,
    pub hits: Vec<SearchHit>,
    pub notes: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct SearchHit {
    pub location: String,
    pub score: Option<String>,
    pub preview: Vec<String>,
}

#[must_use]
pub fn code_search(output: &str) -> Option<SearchView> {
    let mut lines = output.lines();
    let header = lines.next()?.to_owned();
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut notes = Vec::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        if let Some(preview) = line.strip_prefix("    ") {
            if let Some(hit) = hits.last_mut() {
                hit.preview.push(preview.to_owned());
            }
        } else if line.starts_with('(') {
            notes.push(line.trim_matches(|c| c == '(' || c == ')').to_owned());
        } else if let Some((location, rest)) = line.split_once("  (score ") {
            hits.push(SearchHit {
                location: location.to_owned(),
                score: Some(rest.trim_end_matches(')').to_owned()),
                preview: Vec::new(),
            });
        }
    }
    (!hits.is_empty()).then_some(SearchView {
        header,
        hits,
        notes,
    })
}

/// `diagnostics`: a clean bill, or problem lines starting with a severity word.
#[derive(Debug, PartialEq, Eq)]
pub enum Problems {
    Clean(String),
    Found(Vec<String>),
    Unchecked(String),
}

#[must_use]
pub fn diagnostics(output: &str) -> Problems {
    let first = output.lines().next().unwrap_or_default();
    if first.starts_with("No errors") {
        return Problems::Clean(first.to_owned());
    }
    if first.starts_with("Couldn't check") {
        return Problems::Unchecked(first.to_owned());
    }
    Problems::Found(
        output
            .lines()
            .map(str::to_owned)
            .filter(|l| !l.trim().is_empty())
            .collect(),
    )
}

/// Lines of the form `Key: value`, as `repo_clone` and `repo_overview` write them.
#[must_use]
pub fn key_values(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(": ")?;
            (!key.contains(' ') || key.len() < 24)
                .then(|| (key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_rows_and_the_total_note() {
        let v = read("     1\tfn main() {}\n     2\t\n\n[2 lines total]").unwrap();
        assert_eq!(v.rows, [(1, "fn main() {}".to_owned()), (2, String::new())]);
        assert_eq!(v.notes, ["[2 lines total]"]);
    }

    #[test]
    fn grep_groups_consecutive_hits_by_file_and_keeps_notes() {
        let v = grep("a.rs:3:let x\na.rs:9:x()\nb/c.rs:1:// x\n... 4 more matches (limit 100)")
            .unwrap();
        assert_eq!(v.files.len(), 2);
        assert_eq!(
            v.files[0].1,
            [(3, "let x".to_owned()), (9, "x()".to_owned())]
        );
        assert_eq!(v.notes, ["... 4 more matches (limit 100)"]);
    }

    #[test]
    fn grep_handles_a_drive_letter_and_colons_in_the_text() {
        let (path, line, text) = split_hit("C:/proj/a.rs:12:let a: u8 = 1;").unwrap();
        assert_eq!((path, line, text), ("C:/proj/a.rs", 12, "let a: u8 = 1;"));
    }

    #[test]
    fn grep_with_no_hits_is_not_a_grep_view() {
        assert!(grep("No matches found").is_none());
    }

    #[test]
    fn glob_paths_and_truncation() {
        let v = glob(
            "a.toml\nb/c.toml\n(Results truncated: showing 2 of 9+ — use a more specific pattern.)",
        )
        .unwrap();
        assert_eq!(v.paths, ["a.toml", "b/c.toml"]);
        assert!(v.note.unwrap().starts_with("Results truncated"));
        assert!(glob("No files found").unwrap().paths.is_empty());
    }

    #[test]
    fn ls_rows_carry_depth_and_kind() {
        let v = ls(
            "Listing . (depth=2, gitignore honored, hidden=false):\n📁 src/\n  📄 lib.rs\n📄 README.md\n",
        );
        let rows: Vec<_> = v
            .rows
            .iter()
            .map(|r| (r.depth, r.name.as_str(), r.is_dir))
            .collect();
        assert_eq!(
            rows,
            [
                (0, "src", true),
                (1, "lib.rs", false),
                (0, "README.md", false)
            ]
        );
    }

    #[test]
    fn code_search_blocks_have_location_score_and_preview() {
        let text = "2 results for: agent loop [keyword, 40 files indexed]\n\na.rs:1-30  (score 4.20)\n    fn run() {}\n    // loop\n\nb.rs:5-9  (score 1.10)\n    x\n\n(note)";
        let v = code_search(text).unwrap();
        assert_eq!(v.hits.len(), 2);
        assert_eq!(v.hits[0].location, "a.rs:1-30");
        assert_eq!(v.hits[0].score.as_deref(), Some("4.20"));
        assert_eq!(v.hits[0].preview, ["fn run() {}", "// loop"]);
        assert_eq!(v.notes, ["note"]);
    }

    #[test]
    fn diagnostics_states() {
        assert!(matches!(
            diagnostics("No errors in a.rs (checked by its language server)."),
            Problems::Clean(_)
        ));
        assert!(matches!(
            diagnostics("Couldn't check a.rs: no server"),
            Problems::Unchecked(_)
        ));
        assert!(
            matches!(diagnostics("2 problem(s) in a.rs (checked with cargo):\nERROR [x]\nERROR [y]"), Problems::Found(l) if l.len() == 3)
        );
    }

    #[test]
    fn key_values_read_repository_lines() {
        let kv =
            key_values("Repository ready: rust-lang/cargo\nStatus: cloned\nLocal path: C:/x/y");
        assert_eq!(kv[1], ("Status".to_owned(), "cloned".to_owned()));
        assert_eq!(kv[2], ("Local path".to_owned(), "C:/x/y".to_owned()));
    }
}
