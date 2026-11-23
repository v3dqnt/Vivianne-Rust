//! Agent profiles: a folder with a `profile.json` (at most 64 KiB) and an optional `prompt.md` (at most 256 KiB).
//! They are looked up in `~/.viv/agents/<name>/`, then the repository's `.viv/agents/<name>/`, then the built-ins.
//! A name is lowercase letters, digits, `-` and `_`, at most 64.
//!
//! A repository's profile is text from a stranger (R4.4): it may give an agent tools, but it cannot set
//! `permission_mode`, it must live inside the repository (a link that leaves it is refused), and a profile of the
//! same name in `~/.viv/agents` wins over it.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const MAX_PROFILE_BYTES: u64 = 64 * 1024;
pub const MAX_PROMPT_BYTES: u64 = 256 * 1024;
pub const MAX_NAME_BYTES: usize = 64;

/// How the profile's prompt joins the agent's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    #[default]
    Append,
    Replace,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    description: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    mcp_servers: Vec<String>,
    permission_mode: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    prompt: PromptMode,
}

/// Where a profile came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    User,
    Repository,
    BuiltIn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    /// `*` means all; `!name` removes one.
    pub tools: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub prompt_mode: PromptMode,
    pub prompt: Option<String>,
    pub source: Source,
}

/// A name that is safe as a folder name.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The profiles that need no files: `main` (everything) and `agent` (a copy of the starting agent).
#[must_use]
pub fn built_ins() -> Vec<Profile> {
    let make = |name: &str, description: &str| Profile {
        name: name.to_owned(),
        description: description.to_owned(),
        tools: vec!["*".to_owned()],
        mcp_servers: vec!["*".to_owned()],
        permission_mode: None,
        model: None,
        effort: None,
        prompt_mode: PromptMode::Append,
        prompt: None,
        source: Source::BuiltIn,
    };
    vec![
        make("main", "Vivianne itself, with every tool."),
        make("agent", "A copy of the agent that started it."),
    ]
}

/// Reads one profile folder. `repository` is the repository root when the folder is a repository's.
pub fn load_dir(dir: &Path, source: Source, repository: Option<&Path>) -> Result<Profile, String> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    if !valid_name(&name) {
        return Err(format!(
            "`{name}` is not a profile name (lowercase letters, digits, - and _, at most {MAX_NAME_BYTES})"
        ));
    }
    if let Some(root) = repository {
        // The folder and the files in it must be inside the repository once links are followed.
        let real = dir.canonicalize().map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        if !real.starts_with(&root)
            || dir
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(format!(
                "profile `{name}` is a link that leaves the repository, so it was refused"
            ));
        }
    }
    let file = dir.join("profile.json");
    let meta =
        std::fs::metadata(&file).map_err(|_| format!("profile `{name}` has no profile.json"))?;
    if meta.len() > MAX_PROFILE_BYTES {
        return Err(format!(
            "profile `{name}`: profile.json is over {MAX_PROFILE_BYTES} bytes"
        ));
    }
    let raw: Raw =
        serde_json::from_str(&std::fs::read_to_string(&file).map_err(|e| e.to_string())?)
            .map_err(|e| format!("profile `{name}`: {e}"))?;
    let description = raw
        .description
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| format!("profile `{name}` has no description, so it does not load"))?;
    if source == Source::Repository && raw.permission_mode.is_some() {
        return Err(format!(
            "profile `{name}`: a repository profile cannot set permission_mode"
        ));
    }
    let prompt = match std::fs::metadata(dir.join("prompt.md")) {
        Ok(m) if m.len() > MAX_PROMPT_BYTES => {
            return Err(format!(
                "profile `{name}`: prompt.md is over {MAX_PROMPT_BYTES} bytes"
            ));
        }
        Ok(_) => std::fs::read_to_string(dir.join("prompt.md")).ok(),
        Err(_) => None,
    };
    Ok(Profile {
        name,
        description,
        tools: raw.tools,
        mcp_servers: raw.mcp_servers,
        permission_mode: raw.permission_mode,
        model: raw.model,
        effort: raw.effort,
        prompt_mode: raw.prompt,
        prompt,
        source,
    })
}

fn read_all(
    root: &Path,
    source: Source,
    repository: Option<&Path>,
    problems: &mut Vec<String>,
) -> Vec<Profile> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    // A folder with no profile.json is not a profile (the folder may hold something else, such as an installed
    // coding agent), so it is skipped without comment.
    for entry in entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir() && e.path().join("profile.json").is_file())
    {
        match load_dir(&entry.path(), source, repository) {
            Ok(p) => found.push(p),
            Err(why) => problems.push(why),
        }
    }
    found
}

/// Every profile visible from `workspace`, in lookup order and without repeats (the first of a name wins), with
/// the problems found in profiles that did not load.
#[must_use]
pub fn list_with(user_root: &Path, workspace: &Path) -> (Vec<Profile>, Vec<String>) {
    let mut problems = Vec::new();
    let mut all = read_all(user_root, Source::User, None, &mut problems);
    all.extend(read_all(
        &workspace.join(".viv").join("agents"),
        Source::Repository,
        Some(workspace),
        &mut problems,
    ));
    all.extend(built_ins());
    let mut seen = std::collections::HashSet::new();
    all.retain(|p| seen.insert(p.name.clone()));
    (all, problems)
}

/// The profiles for `workspace`, from the real `~/.viv/agents`.
#[must_use]
pub fn list(workspace: &Path) -> (Vec<Profile>, Vec<String>) {
    list_with(&user_root(), workspace)
}

#[must_use]
pub fn user_root() -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["agents"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-profiles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("creates");
        dir
    }

    fn profile(root: &Path, name: &str, json: &str, prompt: Option<&str>) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("creates");
        std::fs::write(dir.join("profile.json"), json).expect("writes");
        if let Some(p) = prompt {
            std::fs::write(dir.join("prompt.md"), p).expect("writes");
        }
    }

    #[test]
    fn names_are_lowercase_words_and_a_description_is_required() {
        for ok in ["reviewer", "doc_writer", "a-1"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "Reviewer", "a b", "../x", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
        let root = temp();
        profile(&root, "nodesc", r#"{"tools": ["read"]}"#, None);
        assert!(
            load_dir(&root.join("nodesc"), Source::User, None)
                .unwrap_err()
                .contains("no description")
        );
        profile(
            &root,
            "good",
            r#"{"description": "reads", "tools": ["read", "!shell"], "prompt": "replace"}"#,
            Some("Be brief."),
        );
        let p = load_dir(&root.join("good"), Source::User, None).expect("loads");
        assert_eq!(
            (p.prompt_mode, p.prompt.as_deref(), p.tools.len()),
            (PromptMode::Replace, Some("Be brief."), 2)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_repository_profile_cannot_set_permission_mode_and_unknown_keys_are_refused() {
        let repo = temp();
        let agents = repo.join(".viv").join("agents");
        profile(
            &agents,
            "sneaky",
            r#"{"description": "x", "permission_mode": "auto"}"#,
            None,
        );
        profile(
            &agents,
            "typo",
            r#"{"description": "x", "toools": []}"#,
            None,
        );
        profile(
            &agents,
            "fine",
            r#"{"description": "x", "tools": ["read"]}"#,
            None,
        );
        let user = temp();
        let (all, problems) = list_with(&user, &repo);
        assert!(
            all.iter()
                .any(|p| p.name == "fine" && p.source == Source::Repository)
        );
        assert!(!all.iter().any(|p| p.name == "sneaky" || p.name == "typo"));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("cannot set permission_mode"))
        );
        assert!(problems.iter().any(|p| p.contains("toools")));
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn the_users_profile_wins_over_a_repositorys_of_the_same_name_and_the_built_ins_are_always_there()
     {
        let repo = temp();
        let user = temp();
        profile(
            &repo.join(".viv").join("agents"),
            "reviewer",
            r#"{"description": "from the repo"}"#,
            None,
        );
        profile(&user, "reviewer", r#"{"description": "from me"}"#, None);
        let (all, _) = list_with(&user, &repo);
        let reviewer = all.iter().find(|p| p.name == "reviewer").expect("found");
        assert_eq!(
            (reviewer.description.as_str(), reviewer.source),
            ("from me", Source::User)
        );
        assert!(
            all.iter()
                .any(|p| p.name == "main" && p.source == Source::BuiltIn)
        );
        assert!(all.iter().any(|p| p.name == "agent"));
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn a_folder_that_is_not_a_profile_is_skipped_without_a_problem() {
        let user = temp();
        std::fs::create_dir_all(user.join(".viv")).expect("creates");
        std::fs::create_dir_all(user.join("claude-acp")).expect("creates");
        let (all, problems) = list_with(&user, &temp());
        assert!(
            problems.is_empty() && all.len() == 2,
            "only the built-ins: {all:?} {problems:?}"
        );
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn an_oversized_profile_or_prompt_is_refused() {
        let root = temp();
        let big = format!(
            r#"{{"description": "x", "tools": ["{}"]}}"#,
            "a".repeat(usize::try_from(MAX_PROFILE_BYTES).expect("fits"))
        );
        profile(&root, "big", &big, None);
        assert!(
            load_dir(&root.join("big"), Source::User, None)
                .unwrap_err()
                .contains("over")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
