//! What kinds of subagent there are: a name, what it is for, whether it may change files, and a few lines added to
//! its system prompt. Three ship with the app; more are markdown files in `~/.viv/agent/subagents/` (the old app's
//! folder): a `---` header with `name`, `description`, `read_only` and `default_prompt`, then the prompt itself.
//! A header key this reads not is ignored, so the old app's richer files still load.

use std::path::Path;

/// One kind of subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    /// It may look but not change anything, whatever mode its parent is in.
    pub read_only: bool,
    pub system: String,
    /// What it does when asked without a prompt.
    pub default_prompt: Option<String>,
    pub builtin: bool,
}

const REVIEWER: &str = "You are reviewing code for defects. Read the change and enough of the surrounding code to judge it in context: a diff alone hides most real bugs.\n\nReport only findings you can justify concretely. For each: the file and line, what breaks, and the specific input or state that triggers it. Rank by severity.\n\nPrioritise, in order: correctness bugs (wrong results, unhandled errors, races, off-by-one, missing-value paths), security issues (injection, path traversal, leaked secrets, missing authorisation), then contradictions with the conventions already established in this codebase.\n\nDo not pad the review. \"No defects found\" is a valid and useful result: say it plainly instead of inventing nitpicks. Style preferences the surrounding code does not already enforce are not findings.";

fn builtins() -> Vec<Profile> {
    let make =
        |name: &str, description: &str, read_only, system: &str, default_prompt: Option<&str>| {
            Profile {
                name: name.to_owned(),
                description: description.to_owned(),
                read_only,
                system: system.to_owned(),
                default_prompt: default_prompt.map(str::to_owned),
                builtin: true,
            }
        };
    vec![
        make(
            "general-purpose",
            "Researches questions, searches code and carries out multi-step work. It may change files when you are in Auto mode.",
            false,
            "",
            None,
        ),
        make(
            "explore",
            "Read-only search: finds files, traces call sites, answers \"where is X\", without changing anything.",
            true,
            "You search and report. You cannot change anything. Answer with what you found: paths and line numbers first, then a short explanation.",
            None,
        ),
        make(
            "code-reviewer",
            "Reviews a change for real defects and reports them. Never edits.",
            true,
            REVIEWER,
            Some(
                "Review the current uncommitted changes (`git diff HEAD`, plus untracked files) for defects.",
            ),
        ),
    ]
}

/// The value of `key: value` in a header line.
fn field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim().trim_matches(['"', '\'']))
    })
}

/// A profile from a markdown file's text; `fallback_name` is the file's name without its extension.
#[must_use]
pub fn parse(text: &str, fallback_name: &str) -> Option<Profile> {
    let text = text.trim_start_matches('\u{feff}');
    let rest = text.strip_prefix("---")?;
    let (header, body) = rest.split_once("\n---")?;
    let name = field(header, "name")
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback_name);
    Some(Profile {
        name: name.to_owned(),
        description: field(header, "description").unwrap_or_default().to_owned(),
        read_only: field(header, "read_only").is_some_and(|v| v == "true"),
        system: body.trim().to_owned(),
        default_prompt: field(header, "default_prompt")
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
        builtin: false,
    })
}

fn from_dir(dir: &Path) -> Vec<Profile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Profile> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            parse(&std::fs::read_to_string(&path).ok()?, &stem)
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Every profile: the user's first, so a file can stand in for a built-in of the same name.
#[must_use]
pub fn profiles() -> Vec<Profile> {
    let mut all = from_dir(&vivianne_ai::paths::viv_home_path(&["agent", "subagents"]));
    for builtin in builtins() {
        if !all
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&builtin.name))
        {
            all.push(builtin);
        }
    }
    all
}

/// The profile called `name`, in any case.
#[must_use]
pub fn find(name: &str) -> Option<Profile> {
    profiles()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_file_is_read_and_extra_header_keys_are_ignored() {
        let text = "---\nname: debugger\ndescription: Finds the cause.\nread_only: true\ncolor: amber\nallowed_tools:\n  - read\n---\nYou diagnose; you do not patch.\n";
        let p = parse(text, "file-name").expect("parses");
        assert_eq!(
            (p.name.as_str(), p.read_only, p.system.as_str()),
            ("debugger", true, "You diagnose; you do not patch.")
        );
        assert_eq!(
            parse("---\ndescription: x\n---\nbody", "from-file").map(|p| p.name),
            Some("from-file".to_owned())
        );
        assert!(parse("no header", "x").is_none());
    }

    #[test]
    fn the_built_ins_are_there_and_the_read_only_ones_say_so() {
        let all = builtins();
        let by = |n: &str| all.iter().find(|p| p.name == n);
        assert_eq!(by("general-purpose").map(|p| p.read_only), Some(false));
        assert_eq!(by("explore").map(|p| p.read_only), Some(true));
        assert!(by("code-reviewer").is_some_and(|p| p.read_only && p.default_prompt.is_some()));
    }
}
