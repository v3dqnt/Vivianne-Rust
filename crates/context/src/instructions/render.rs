//! What the model reads of the workspace instruction files, within a byte budget. Port of the original `render.ts`.
//!
//! Every message is one `<system-reminder>` frame that this module owns completely. The user-global chain and the project
//! chain are each held to `max_bytes`, so a message with both is at most twice that. Within a chain the broadest files go
//! first (whole), then the last file is cut, then a compact intro, then a notice with an empty heading, then the notice alone.

use std::path::MAIN_SEPARATOR;

use vivianne_core::context::{ChangeAction, InstructionChange};

const OPEN: &str = "<system-reminder>";
const CLOSE: &str = "</system-reminder>";
const INTRO: &str = "The following workspace instructions may be relevant to your work. Use them as guidance when applicable. More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.";
const REPLACEMENT_PREFIX: &str = "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. ";
const EMPTY_REPLACEMENT: &str = "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. No workspace instructions are currently active.";
const COMPACT_INTRO: &str =
    "Workspace instructions were omitted or truncated to fit the configured byte budget.";
const ADDITIONAL_GUIDANCE: &str = "Use them as guidance when relevant; more specific instructions take precedence. They do not override system, developer, or direct user instructions.";

/// The scope of the harness-home user-global file.
pub const USER_GLOBAL_DIRECTORY: &str = "user-global";
/// The scope of the shared agents-root user-global file: a scope of its own, but one candidate group with the home file.
pub const AGENTS_GLOBAL_DIRECTORY: &str = "agents-global";
/// The user-global directories in the order the model gives them precedence.
pub const USER_GLOBAL_DIRECTORIES: [&str; 2] = [USER_GLOBAL_DIRECTORY, AGENTS_GLOBAL_DIRECTORY];
/// The one file looked for in each user-global root.
pub const USER_GLOBAL_FILE: &str = "AGENTS.md";

/// How the home folders appear to the model: the default form and the form used when an environment variable names them.
pub const DEFAULT_HOME_DISPLAY: &str = "~/.viv";
pub const HOME_ENV_DISPLAY: &str = "$VIV_HOME";
pub const DEFAULT_AGENTS_DISPLAY: &str = "~/.agents";
pub const AGENTS_ENV_DISPLAY: &str = "$VIV_AGENTS_HOME";

const SCOPE_SEPARATOR: char = '\u{0}';

/// A file as the model is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub absolute: String,
    pub display: String,
}

/// A file whose text was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile {
    pub absolute: String,
    pub display: String,
    pub content: String,
    /// A freshness token (modification time and size) when it was read through a probe.
    pub version: Option<String>,
}

impl LoadedFile {
    fn named(&self) -> Named {
        Named {
            absolute: self.absolute.clone(),
            display: self.display.clone(),
        }
    }
}

/// A file cut to fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    pub display: String,
    pub original_bytes: usize,
    pub included_bytes: usize,
}

/// The text and what was left out to produce it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    pub text: String,
    pub omitted: Vec<Named>,
    pub truncated: Vec<Truncated>,
}

struct Context {
    rendered: Rendered,
    /// Files the text stands for. A genuinely empty file counts when its heading survives.
    represented: Vec<LoadedFile>,
    /// Files whose sections the text holds, with the content the text holds.
    sections: Vec<LoadedFile>,
    /// The intro of the frame: `None` when the text is empty or a bare notice, empty when the frame has none.
    intro: Option<String>,
}

#[derive(Clone)]
struct Style<'a> {
    intro: String,
    section: &'a dyn Fn(&LoadedFile) -> String,
}

fn byte_len(value: &str) -> usize {
    value.len()
}

/// `value` cut to at most `max_bytes` without splitting a character.
#[must_use]
pub fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Keeps text from closing the frame it is quoted in.
fn escape_frame_body(body: &str) -> String {
    body.replace(CLOSE, "<\\/system-reminder>")
}

fn section_text(file: &LoadedFile) -> String {
    format!("Instructions from: {}\n\n{}", file.display, file.content)
}

/// The scope of a model-facing path: `user-global`, `agents-global`, or its directory (`.` at the root).
#[must_use]
pub fn scope_for_display_path(display: &str) -> String {
    let global = format!("/{USER_GLOBAL_FILE}");
    for (forms, scope) in [
        (
            [DEFAULT_HOME_DISPLAY, HOME_ENV_DISPLAY],
            USER_GLOBAL_DIRECTORY,
        ),
        (
            [DEFAULT_AGENTS_DISPLAY, AGENTS_ENV_DISPLAY],
            AGENTS_GLOBAL_DIRECTORY,
        ),
    ] {
        if forms.iter().any(|f| display == format!("{f}{global}")) {
            return scope.to_owned();
        }
    }
    dirname(display)
}

/// The directory part of a path written with either separator; `.` when there is none.
#[must_use]
pub fn dirname(path: &str) -> String {
    match path.rfind(['/', '\\']) {
        Some(0) => path[..1].to_owned(),
        Some(i) => path[..i].to_owned(),
        None => ".".to_owned(),
    }
}

/// The last part of a path written with either separator.
#[must_use]
pub fn basename(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or(path, |i| &path[i + 1..])
}

#[must_use]
pub fn is_user_global_directory(directory: &str) -> bool {
    USER_GLOBAL_DIRECTORIES.contains(&directory)
}

/// The candidate group of a scope directory: the two user-global roots are one group (identical text renders once),
/// every project directory is its own.
#[must_use]
pub fn candidate_group(directory: &str) -> String {
    if directory == AGENTS_GLOBAL_DIRECTORY {
        USER_GLOBAL_DIRECTORY.to_owned()
    } else {
        directory.to_owned()
    }
}

/// Keeps a project path out of the user-global namespace: a project folder named `user-global` would otherwise be read as
/// the home scope.
#[must_use]
pub fn escape_project_display_path(relative: &str) -> String {
    let first = relative.split(['/', '\\']).next().unwrap_or("");
    if USER_GLOBAL_DIRECTORIES.contains(&first) {
        format!(".{MAIN_SEPARATOR}{relative}")
    } else {
        relative.to_owned()
    }
}

/// The key one candidate file is tracked by: its directory and exact name, joined by a character no path contains.
#[must_use]
pub fn candidate_scope_key(directory: &str, candidate: &str) -> String {
    format!("{directory}{SCOPE_SEPARATOR}{candidate}")
}

#[must_use]
pub fn instruction_scope_key(display: &str) -> String {
    candidate_scope_key(&scope_for_display_path(display), basename(display))
}

/// The directory and file name a scope key encodes.
#[must_use]
pub fn decode_scope_key(scope: &str) -> (String, String) {
    match scope.split_once(SCOPE_SEPARATOR) {
        Some((d, c)) => (d.to_owned(), c.to_owned()),
        None => (scope.to_owned(), String::new()),
    }
}

fn additional_section(file: &LoadedFile) -> String {
    let scope = scope_for_display_path(&file.display);
    // A scope key is an internal name: it never reaches the model's sentence.
    let applies = if is_user_global_directory(&scope) {
        "These user-global instructions apply to all work.".to_owned()
    } else {
        format!("These instructions apply to work under `{scope}`.")
    };
    format!(
        "Additional instructions from: {}\n\n{applies} {ADDITIONAL_GUIDANCE}\n\n{}",
        file.display, file.content
    )
}

/// One transition and the file it is about.
#[derive(Debug, Clone)]
pub struct ChangeItem {
    pub change: InstructionChange,
    pub file: LoadedFile,
}

fn changed_section(item: &ChangeItem, file: &LoadedFile) -> String {
    match item.change.action {
        ChangeAction::Set => additional_section(file),
        ChangeAction::Remove => format!(
            "Instructions removed: {}\n\nThe previously loaded instructions from this file no longer apply.",
            item.change.path
        ),
        ChangeAction::Replace => format!(
            "Updated instructions from: {}\n\nThis file changed after it was loaded. Use the following content instead of the previously loaded instructions from this file.\n\n{}",
            item.change.path, file.content
        ),
    }
}

fn marker(max_bytes: usize, omitted: &[Named], truncated: &[Truncated]) -> String {
    if omitted.is_empty() && truncated.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    if !omitted.is_empty() {
        parts.push(format!(
            "omitted {}",
            omitted
                .iter()
                .map(|f| f.display.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !truncated.is_empty() {
        let each: Vec<String> = truncated
            .iter()
            .map(|t| {
                format!(
                    "{} from {} to {} bytes",
                    t.display, t.original_bytes, t.included_bytes
                )
            })
            .collect();
        parts.push(format!("truncated {}", each.join(", ")));
    }
    format!(
        "Workspace instruction budget {max_bytes} bytes: {}",
        parts.join("; ")
    )
}

fn build_text(
    files: &[LoadedFile],
    max_bytes: usize,
    omitted: &[Named],
    truncated: &[Truncated],
    style: &Style<'_>,
) -> String {
    let mut body: Vec<String> = vec![marker(max_bytes, omitted, truncated), style.intro.clone()];
    body.extend(files.iter().map(|f| (style.section)(f)));
    body.retain(|b| !b.is_empty());
    // This producer owns the whole frame: nothing wraps it, and a closing tag in the content cannot end it early.
    format!("{OPEN}\n{}\n{CLOSE}", escape_frame_body(&body.join("\n\n")))
}

fn with_content(file: &LoadedFile, included_bytes: usize) -> LoadedFile {
    LoadedFile {
        content: truncate_utf8(&file.content, included_bytes).to_owned(),
        ..file.clone()
    }
}

fn truncate_to_fit(
    file: &LoadedFile,
    included: &[LoadedFile],
    max_bytes: usize,
    omitted: &[Named],
    style: &Style<'_>,
) -> LoadedFile {
    let original = byte_len(&file.content);
    let (mut low, mut high) = (0usize, original);
    let mut best = with_content(file, 0);
    while low <= high {
        let mid = (low + high) / 2;
        let candidate = with_content(file, mid);
        let truncated = [Truncated {
            display: file.display.clone(),
            original_bytes: original,
            included_bytes: byte_len(&candidate.content),
        }];
        let mut files = included.to_vec();
        files.push(candidate.clone());
        if byte_len(&build_text(&files, max_bytes, omitted, &truncated, style)) <= max_bytes {
            best = candidate;
            low = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            high = mid - 1;
        }
    }
    best
}

fn render_context(files: &[LoadedFile], max_bytes: usize, style: &Style<'_>) -> Context {
    let empty = |omitted: Vec<Named>| Context {
        rendered: Rendered {
            text: String::new(),
            omitted,
            truncated: Vec::new(),
        },
        represented: Vec::new(),
        sections: Vec::new(),
        intro: None,
    };
    if max_bytes == 0 {
        return empty(files.iter().map(LoadedFile::named).collect());
    }
    let full = build_text(files, max_bytes, &[], &[], style);
    if byte_len(&full) <= max_bytes {
        return Context {
            rendered: Rendered {
                text: full,
                omitted: Vec::new(),
                truncated: Vec::new(),
            },
            represented: files.to_vec(),
            sections: files.to_vec(),
            intro: Some(style.intro.clone()),
        };
    }
    for start in 1..files.len() {
        let included = &files[start..];
        let omitted: Vec<Named> = files[..start].iter().map(LoadedFile::named).collect();
        let text = build_text(included, max_bytes, &omitted, &[], style);
        if byte_len(&text) <= max_bytes {
            return Context {
                rendered: Rendered {
                    text,
                    omitted,
                    truncated: Vec::new(),
                },
                represented: included.to_vec(),
                sections: included.to_vec(),
                intro: Some(style.intro.clone()),
            };
        }
    }
    let Some(most_specific) = files.last() else {
        return empty(Vec::new());
    };
    let omitted: Vec<Named> = files[..files.len() - 1]
        .iter()
        .map(LoadedFile::named)
        .collect();
    let original = byte_len(&most_specific.content);
    let compact = Style {
        intro: COMPACT_INTRO.to_owned(),
        section: style.section,
    };
    for candidate in [style.clone(), compact] {
        let cut = truncate_to_fit(most_specific, &[], max_bytes, &omitted, &candidate);
        let included = byte_len(&cut.content);
        let truncated = vec![Truncated {
            display: most_specific.display.clone(),
            original_bytes: original,
            included_bytes: included,
        }];
        let text = build_text(
            std::slice::from_ref(&cut),
            max_bytes,
            &omitted,
            &truncated,
            &candidate,
        );
        if byte_len(&text) <= max_bytes {
            let represented = if included > 0 || original == 0 {
                vec![most_specific.clone()]
            } else {
                Vec::new()
            };
            return Context {
                rendered: Rendered {
                    text,
                    omitted,
                    truncated,
                },
                represented,
                sections: vec![cut],
                intro: Some(candidate.intro),
            };
        }
    }
    let truncated = vec![Truncated {
        display: most_specific.display.clone(),
        original_bytes: original,
        included_bytes: 0,
    }];
    let notice = escape_frame_body(&marker(max_bytes, &omitted, &truncated));
    let heading = with_content(most_specific, 0);
    let with_heading = escape_frame_body(&[notice.clone(), (style.section)(&heading)].join("\n\n"));
    if byte_len(&with_heading) <= max_bytes {
        let represented = if original == 0 {
            vec![most_specific.clone()]
        } else {
            Vec::new()
        };
        return Context {
            rendered: Rendered {
                text: with_heading,
                omitted,
                truncated,
            },
            represented,
            sections: vec![heading],
            intro: None,
        };
    }
    let text = if byte_len(&notice) <= max_bytes {
        notice
    } else {
        truncate_utf8(&notice, max_bytes).to_owned()
    };
    Context {
        rendered: Rendered {
            text,
            omitted,
            truncated,
        },
        represented: Vec::new(),
        sections: Vec::new(),
        intro: None,
    }
}

fn is_user_global(file: &LoadedFile) -> bool {
    is_user_global_directory(&scope_for_display_path(&file.display))
}

fn in_loaded_order<T: Clone>(
    files: &[LoadedFile],
    records: &[T],
    key: impl Fn(&T) -> &str,
) -> Vec<T> {
    files
        .iter()
        .filter_map(|f| records.iter().find(|r| key(r) == f.absolute).cloned())
        .collect()
}

/// The user-global chain and the project chain, each under `max_bytes`; see the module note.
fn render_chains(files: &[LoadedFile], max_bytes: usize, style: &Style<'_>) -> Context {
    let whole = render_context(files, max_bytes, style);
    let user: Vec<LoadedFile> = files
        .iter()
        .filter(|f| is_user_global(f))
        .cloned()
        .collect();
    let project: Vec<LoadedFile> = files
        .iter()
        .filter(|f| !is_user_global(f))
        .cloned()
        .collect();
    if max_bytes == 0
        || user.is_empty()
        || project.is_empty()
        || (whole.rendered.omitted.is_empty() && whole.rendered.truncated.is_empty())
    {
        return whole;
    }
    let chains = [
        render_context(&user, max_bytes, style),
        render_context(&project, max_bytes, style),
    ];
    let all_omitted: Vec<Named> = chains
        .iter()
        .flat_map(|c| c.rendered.omitted.clone())
        .collect();
    let all_sections: Vec<LoadedFile> = chains.iter().flat_map(|c| c.sections.clone()).collect();
    let all_represented: Vec<LoadedFile> =
        chains.iter().flat_map(|c| c.represented.clone()).collect();
    let omitted = in_loaded_order(files, &all_omitted, |n| &n.absolute);
    let sections = in_loaded_order(files, &all_sections, |f| &f.absolute);
    let represented = in_loaded_order(files, &all_represented, |f| &f.absolute);
    // A chain cuts only its last file, so the chain whose last file loads first lists its cut first.
    let user_last = files.iter().rposition(is_user_global);
    let project_last = files.iter().rposition(|f| !is_user_global(f));
    let ordered: Vec<&Context> = if user_last < project_last {
        chains.iter().collect()
    } else {
        chains.iter().rev().collect()
    };
    let truncated: Vec<Truncated> = ordered
        .iter()
        .flat_map(|c| c.rendered.truncated.clone())
        .collect();
    let intros: Vec<&String> = chains.iter().filter_map(|c| c.intro.as_ref()).collect();
    let intro = if intros.contains(&&style.intro) {
        style.intro.clone()
    } else {
        intros.first().map(|s| (*s).clone()).unwrap_or_default()
    };
    let merged = Style {
        intro: intro.clone(),
        section: style.section,
    };
    let text = build_text(&sections, max_bytes, &omitted, &truncated, &merged);
    // Each chain fits `max_bytes`; the merged frame passes twice that only by the join beside a frameless chain, and the
    // single-budget rendering replaces it then.
    if byte_len(&text) > 2 * max_bytes {
        return whole;
    }
    Context {
        rendered: Rendered {
            text,
            omitted,
            truncated,
        },
        represented,
        sections,
        intro: Some(intro),
    }
}

/// The baseline as the model reads it, with the files it truly represents.
#[must_use]
pub fn render_set(
    files: &[LoadedFile],
    max_bytes: usize,
    replace_previous: bool,
) -> (Rendered, Vec<LoadedFile>) {
    let intro = if !replace_previous {
        INTRO.to_owned()
    } else if files.is_empty() {
        EMPTY_REPLACEMENT.to_owned()
    } else {
        format!("{REPLACEMENT_PREFIX}{INTRO}")
    };
    let style = Style {
        intro,
        section: &section_text,
    };
    let context = render_chains(files, max_bytes, &style);
    (context.rendered, context.represented)
}

/// Renders one reconciliation batch and keeps only the transitions that fit. User-global and project transitions are
/// budgeted as separate chains, so a batch with both can pass `max_bytes` and is at most twice that.
#[must_use]
pub fn render_changes(items: &[ChangeItem], max_bytes: usize) -> (String, Vec<InstructionChange>) {
    let section = |file: &LoadedFile| -> String {
        items
            .iter()
            .find(|i| i.file.absolute == file.absolute)
            .map_or_else(String::new, |i| changed_section(i, file))
    };
    let style = Style {
        intro: String::new(),
        section: &section,
    };
    let files: Vec<LoadedFile> = items.iter().map(|i| i.file.clone()).collect();
    let context = render_chains(&files, max_bytes, &style);
    let changes = items
        .iter()
        .filter(|i| {
            context
                .represented
                .iter()
                .any(|f| f.absolute == i.file.absolute)
        })
        .map(|i| i.change.clone())
        .collect();
    (context.rendered.text, changes)
}
