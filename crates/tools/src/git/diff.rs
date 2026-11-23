//! What the Git tab's diff view does with a diff before drawing it: parsing it into numbered lines, hiding
//! changes that are only imports, marking the words that changed inside a line, and the command that
//! re-applies it. No drawing here, so it can be tested on its own.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Add,
    Del,
    Ctx,
    Hunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// Parses a unified diff into numbered lines.
#[must_use]
pub fn parse(diff: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let (mut old_no, mut new_no) = (0_u32, 0_u32);
    for line in diff.split('\n') {
        if line.starts_with("@@") {
            if let Some((o, n)) = hunk_start(line) {
                (old_no, new_no) = (o, n);
            }
            out.push(Line {
                kind: Kind::Hunk,
                text: line.to_owned(),
                old_no: None,
                new_no: None,
            });
        } else if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with('\\')
        {
            // File headers and "\ No newline at end of file": not lines of the file.
        } else if let Some(text) = line.strip_prefix('+') {
            out.push(Line {
                kind: Kind::Add,
                text: text.to_owned(),
                old_no: None,
                new_no: Some(new_no),
            });
            new_no += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            out.push(Line {
                kind: Kind::Del,
                text: text.to_owned(),
                old_no: Some(old_no),
                new_no: None,
            });
            old_no += 1;
        } else {
            let text = line.get(1..).unwrap_or("");
            out.push(Line {
                kind: Kind::Ctx,
                text: text.to_owned(),
                old_no: Some(old_no),
                new_no: Some(new_no),
            });
            old_no += 1;
            new_no += 1;
        }
    }
    // The blank after the final newline.
    if out
        .last()
        .is_some_and(|l| l.kind == Kind::Ctx && l.text.is_empty())
    {
        out.pop();
    }
    out
}

/// `@@ -12,5 +14,6 @@` gives (12, 14).
fn hunk_start(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let new = rest.split(" @@").next()?;
    let first = |s: &str| s.split(',').next()?.parse().ok();
    Some((first(old)?, first(new)?))
}

/// A line that opens an import, and whether it stays open across later lines.
fn import_start(text: &str, path: &str) -> (bool, bool) {
    let t = text.trim();
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            if t.starts_with("import") && t.contains('{') && !t.contains('}') {
                (true, true)
            } else {
                let is = t.starts_with("import ")
                    || t.starts_with("import{")
                    || (t.starts_with("export ") && t.contains(" from "))
                    || (t.contains("= require(")
                        && (t.starts_with("const ")
                            || t.starts_with("let ")
                            || t.starts_with("var ")));
                (is, false)
            }
        }
        "rs" => {
            let rest = t.strip_prefix("pub ").map_or(t, |r| {
                r.split_once(") ")
                    .filter(|_| r.starts_with('('))
                    .map_or(r, |(_, after)| after)
            });
            if rest.starts_with("use ") {
                (true, rest.contains('{') && !rest.contains('}'))
            } else {
                (false, false)
            }
        }
        "py" => (
            t.starts_with("import ") || (t.starts_with("from ") && t.contains(" import ")),
            false,
        ),
        "go" => (t.starts_with("import"), false),
        _ => (false, false),
    }
}

/// The diff without the changes that are only imports. An import written over several lines goes whole; a
/// hunk left with nothing changed goes too.
#[must_use]
pub fn hide_imports(lines: &[Line], path: &str) -> Vec<Line> {
    let mut kept = Vec::new();
    let mut in_import = false;
    for l in lines {
        if matches!(l.kind, Kind::Hunk | Kind::Ctx) {
            kept.push(l.clone());
            continue;
        }
        if in_import {
            if l.text.contains('}') {
                in_import = false;
            }
            continue;
        }
        let (is, open) = import_start(&l.text, path);
        if is {
            in_import = open;
            continue;
        }
        kept.push(l.clone());
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < kept.len() {
        if kept[i].kind != Kind::Hunk {
            out.push(kept[i].clone());
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < kept.len() && kept[j].kind != Kind::Hunk {
            j += 1;
        }
        if kept[i + 1..j]
            .iter()
            .any(|l| matches!(l.kind, Kind::Add | Kind::Del))
        {
            out.extend_from_slice(&kept[i..j]);
        }
        i = j;
    }
    out
}

/// A run of text that did or did not change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub changed: bool,
}

/// Past this many tokens a line is not worth comparing word by word.
const MAX_TOKENS: usize = 300;

/// Words, whitespace runs and single symbols.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut class = None;
    for (i, c) in text.char_indices() {
        let now = if c.is_whitespace() {
            0
        } else if c.is_alphanumeric() || c == '_' {
            1
        } else {
            2
        };
        // Symbols stand alone; words and whitespace group.
        if class != Some(now) || now == 2 {
            if i > start {
                out.push(&text[start..i]);
            }
            start = i;
            class = Some(now);
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// The words that differ between an old and a new line, as marked segments; `None` for a line too long to
/// compare.
#[must_use]
pub fn word_segments(old: &str, new: &str) -> Option<(Vec<Seg>, Vec<Seg>)> {
    let (a, b) = (tokens(old), tokens(new));
    if a.len() > MAX_TOKENS || b.len() > MAX_TOKENS {
        return None;
    }
    // Longest common subsequence of tokens.
    let mut lcs = vec![vec![0_u16; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut same_a, mut same_b) = (vec![false; a.len()], vec![false; b.len()]);
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            same_a[i] = true;
            same_b[j] = true;
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let join = |tokens: &[&str], same: &[bool]| {
        let mut segs: Vec<Seg> = Vec::new();
        for (token, same) in tokens.iter().zip(same) {
            let changed = !same;
            match segs.last_mut() {
                Some(last) if last.changed == changed => last.text.push_str(token),
                _ => segs.push(Seg {
                    text: (*token).to_owned(),
                    changed,
                }),
            }
        }
        segs
    };
    Some((join(&a, &same_a), join(&b, &same_b)))
}

/// Word segments for the lines of each changed run: the nth removed line is compared with the nth added line,
/// which is how an edited line sits opposite the one it replaced. Indexed like `lines`.
#[must_use]
pub fn word_diffs(lines: &[Line]) -> std::collections::HashMap<usize, Vec<Seg>> {
    let mut out = std::collections::HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != Kind::Del {
            i += 1;
            continue;
        }
        let (mut dels, mut adds) = (Vec::new(), Vec::new());
        while i < lines.len() && lines[i].kind == Kind::Del {
            dels.push(i);
            i += 1;
        }
        while i < lines.len() && lines[i].kind == Kind::Add {
            adds.push(i);
            i += 1;
        }
        for (d, a) in dels.iter().zip(&adds) {
            let Some((old, new)) = word_segments(&lines[*d].text, &lines[*a].text) else {
                continue;
            };
            // A line replaced wholesale is not a word edit: marking all of it says nothing.
            if old.iter().all(|s| s.changed) && new.iter().all(|s| s.changed) {
                continue;
            }
            out.insert(*d, old);
            out.insert(*a, new);
        }
    }
    out
}

/// The command that applies a patch to a checkout, to paste into a shell.
#[must_use]
pub fn apply_command(patch: &str) -> String {
    let body = if patch.ends_with('\n') {
        patch.to_owned()
    } else {
        format!("{patch}\n")
    };
    format!("git apply <<'VIV_PATCH'\n{body}VIV_PATCH")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/a.ts b/a.ts\nindex 1..2 100644\n--- a/a.ts\n+++ b/a.ts\n@@ -1,3 +1,4 @@\n import a from 'a'\n-const x = 1\n+import b from 'b'\n+const x = 2\n keep\n";

    #[test]
    fn a_diff_is_numbered_from_its_hunk_header() {
        let lines = parse(DIFF);
        assert_eq!(lines[0].kind, Kind::Hunk);
        assert_eq!((lines[1].old_no, lines[1].new_no), (Some(1), Some(1)));
        assert_eq!((lines[2].kind, lines[2].old_no), (Kind::Del, Some(2)));
        assert_eq!((lines[3].kind, lines[3].new_no), (Kind::Add, Some(2)));
        assert_eq!(lines.last().map(|l| l.text.as_str()), Some("keep"));
    }

    #[test]
    fn imports_are_hidden_and_other_changes_stay() {
        let shown = hide_imports(&parse(DIFF), "a.ts");
        assert!(shown.iter().all(|l| !l.text.starts_with("import b")));
        assert!(
            shown
                .iter()
                .any(|l| l.kind == Kind::Add && l.text == "const x = 2")
        );
    }

    #[test]
    fn a_hunk_of_only_imports_goes_with_them() {
        let only = "@@ -1,1 +1,2 @@\n a\n+use std::io;\n";
        assert!(hide_imports(&parse(only), "x.rs").is_empty());
    }

    #[test]
    fn the_changed_word_is_marked() {
        let (old, new) = word_segments("let x = 1;", "let x = 2;").unwrap_or_default();
        let marked = |s: &[Seg]| {
            s.iter()
                .filter(|s| s.changed)
                .map(|s| s.text.clone())
                .collect::<String>()
        };
        assert_eq!(
            (marked(&old), marked(&new)),
            ("1".to_owned(), "2".to_owned())
        );
    }

    #[test]
    fn a_line_replaced_wholesale_is_not_marked() {
        let lines = parse("@@ -1,1 +1,1 @@\n-abc\n+xyz\n");
        assert!(word_diffs(&lines).is_empty());
        assert_eq!(
            word_diffs(&parse("@@ -1,1 +1,1 @@\n-let a = 1\n+let a = 2\n")).len(),
            2
        );
    }

    #[test]
    fn the_apply_command_wraps_the_patch() {
        assert_eq!(apply_command("x"), "git apply <<'VIV_PATCH'\nx\nVIV_PATCH");
    }
}
