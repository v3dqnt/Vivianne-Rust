//! Configuration in layers, highest first: a command-line flag, an environment variable, the workspace's entry in
//! `~/.viv/agent/settings.json`, the top level of that file, the project's `<workspace>/.viv.json`, then the
//! built-in default. Every value says where it came from, so `viv config list --show-origin` can show it.
//!
//! `.viv.json` is shared with a repository, so it is read as data and may set only the repo-safe fields (R4.4):
//! `max_agent_steps`, `max_tool_result_bytes` and `context`. Anything else in it, and above all anything about
//! permissions, is ignored with a warning.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Largest `.viv.json` read.
pub const MAX_PROJECT_FILE_BYTES: u64 = 64 * 1024;
pub const DEFAULT_MAX_AGENT_STEPS: usize = 100;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 65_536;
pub const MIN_RESULT_BYTES: usize = 1024;

/// The fields a repository may set.
const REPO_SAFE: [&str; 3] = ["max_agent_steps", "max_tool_result_bytes", "context"];

/// Where a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Env(String),
    WorkspaceSettings,
    Settings,
    Project(PathBuf),
    Default,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flag => f.write_str("command-line flag"),
            Self::Env(name) => write!(f, "environment {name}"),
            Self::WorkspaceSettings => f.write_str("settings.json (this workspace)"),
            Self::Settings => f.write_str("settings.json"),
            Self::Project(path) => write!(f, "{}", path.display()),
            Self::Default => f.write_str("default"),
        }
    }
}

/// A value and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting<T> {
    pub value: T,
    pub origin: Origin,
}

/// What a project file says, after the repo-safe filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectConfig {
    pub max_agent_steps: Option<usize>,
    pub max_tool_result_bytes: Option<usize>,
    pub context: Option<String>,
    /// What was ignored and why, to show the person.
    pub warnings: Vec<String>,
}

/// Reads the text of a `.viv.json`. Unknown or unsafe fields are dropped with a warning; a file that is not a JSON
/// object is ignored whole.
#[must_use]
pub fn parse_project(text: &str) -> ProjectConfig {
    let mut config = ProjectConfig::default();
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        config
            .warnings
            .push(".viv.json is not a JSON object, so it was ignored.".to_owned());
        return config;
    };
    for (key, value) in &map {
        if !REPO_SAFE.contains(&key.as_str()) {
            config.warnings.push(format!(
                ".viv.json: `{key}` is not allowed in a repository file and was ignored (only {} are)",
                REPO_SAFE.join(", ")
            ));
            continue;
        }
        match (key.as_str(), value) {
            ("max_agent_steps", Value::Number(n)) => {
                config.max_agent_steps = n.as_u64().and_then(|n| usize::try_from(n).ok())
            }
            ("max_tool_result_bytes", Value::Number(n)) => {
                config.max_tool_result_bytes = n
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .map(|n| n.max(MIN_RESULT_BYTES));
            }
            ("context", Value::String(s)) => config.context = Some(s.clone()),
            (other, _) => config.warnings.push(format!(
                ".viv.json: `{other}` has the wrong kind of value and was ignored"
            )),
        }
    }
    config
}

/// The project file of `workspace`, if there is one and it is not too big.
#[must_use]
pub fn load_project(workspace: &Path) -> ProjectConfig {
    let path = workspace.join(".viv.json");
    let Ok(meta) = std::fs::metadata(&path) else {
        return ProjectConfig::default();
    };
    if meta.len() > MAX_PROJECT_FILE_BYTES {
        return ProjectConfig {
            warnings: vec![format!(
                ".viv.json is over {MAX_PROJECT_FILE_BYTES} bytes, so it was ignored."
            )],
            ..ProjectConfig::default()
        };
    }
    std::fs::read_to_string(path)
        .map(|t| parse_project(&t))
        .unwrap_or_default()
}

fn number_of(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|n| usize::try_from(n).ok())
}

/// One number setting through the layers.
#[allow(clippy::too_many_arguments)]
fn resolve_number(
    key: &str,
    env_name: &str,
    flag: Option<usize>,
    env: &dyn Fn(&str) -> Option<String>,
    settings: &Value,
    workspace: &str,
    project: Option<usize>,
    project_path: &Path,
    default: usize,
) -> Setting<usize> {
    if let Some(value) = flag {
        return Setting {
            value,
            origin: Origin::Flag,
        };
    }
    if let Some(value) = env(env_name).and_then(|t| t.trim().parse().ok()) {
        return Setting {
            value,
            origin: Origin::Env(env_name.to_owned()),
        };
    }
    if let Some(value) = settings["workspaces"][workspace]
        .get(key)
        .and_then(number_of)
    {
        return Setting {
            value,
            origin: Origin::WorkspaceSettings,
        };
    }
    if let Some(value) = settings.get(key).and_then(number_of) {
        return Setting {
            value,
            origin: Origin::Settings,
        };
    }
    if let Some(value) = project {
        return Setting {
            value,
            origin: Origin::Project(project_path.to_path_buf()),
        };
    }
    Setting {
        value: default,
        origin: Origin::Default,
    }
}

/// The settings that are numbers, resolved for a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub max_agent_steps: Setting<usize>,
    pub max_tool_result_bytes: Setting<usize>,
    pub warnings: Vec<String>,
}

/// Resolves the settings from the layers it is given (so a test can pass its own).
#[must_use]
pub fn resolve_with(
    workspace: &Path,
    flag_steps: Option<usize>,
    env: &dyn Fn(&str) -> Option<String>,
    settings: &Value,
    project: &ProjectConfig,
) -> Resolved {
    let ws = workspace.display().to_string();
    let project_path = workspace.join(".viv.json");
    let mut bytes = resolve_number(
        "max_tool_result_bytes",
        "VIV_MAX_TOOL_RESULT_BYTES",
        None,
        env,
        settings,
        &ws,
        project.max_tool_result_bytes,
        &project_path,
        DEFAULT_MAX_RESULT_BYTES,
    );
    bytes.value = bytes.value.max(MIN_RESULT_BYTES);
    Resolved {
        max_agent_steps: resolve_number(
            "max_agent_steps",
            "VIV_MAX_AGENT_STEPS",
            flag_steps,
            env,
            settings,
            &ws,
            project.max_agent_steps,
            &project_path,
            DEFAULT_MAX_AGENT_STEPS,
        ),
        max_tool_result_bytes: bytes,
        warnings: project.warnings.clone(),
    }
}

/// The settings for `workspace` as they stand now: the real environment, settings file and project file.
#[must_use]
pub fn resolve(workspace: &Path, flag_steps: Option<usize>) -> Resolved {
    resolve_with(
        workspace,
        flag_steps,
        &|name| std::env::var(name).ok(),
        &vivianne_ai::host::settings_blocking(),
        &load_project(workspace),
    )
}

/// Every setting with its value and origin, for `viv config list --show-origin`.
#[must_use]
pub fn list(resolved: &Resolved) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "max_agent_steps",
            resolved.max_agent_steps.value.to_string(),
            resolved.max_agent_steps.origin.to_string(),
        ),
        (
            "max_tool_result_bytes",
            resolved.max_tool_result_bytes.value.to_string(),
            resolved.max_tool_result_bytes.origin.to_string(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_repository_file_may_set_only_the_repo_safe_fields_and_the_rest_is_dropped_with_a_warning()
    {
        let c = parse_project(
            r#"{"max_agent_steps": 30, "max_tool_result_bytes": 10, "permission_mode": "auto", "hooks": {"x": 1}, "context": "docs/"}"#,
        );
        assert_eq!(c.max_agent_steps, Some(30));
        assert_eq!(
            c.max_tool_result_bytes,
            Some(MIN_RESULT_BYTES),
            "raised to the floor"
        );
        assert_eq!(c.context.as_deref(), Some("docs/"));
        assert_eq!(c.warnings.len(), 2);
        assert!(c.warnings.iter().any(|w| w.contains("permission_mode")));
        assert!(parse_project("[1,2]").warnings[0].contains("not a JSON object"));
        assert!(parse_project("not json").max_agent_steps.is_none());
    }

    #[test]
    fn the_layers_win_in_order_and_each_value_knows_its_origin() {
        let ws = Path::new("C:/work/app");
        let project = parse_project(r#"{"max_agent_steps": 10}"#);
        let settings =
            json!({"max_agent_steps": 20, "workspaces": {"C:/work/app": {"max_agent_steps": 25}}});
        let env = |n: &str| (n == "VIV_MAX_AGENT_STEPS").then(|| "30".to_owned());

        let all = resolve_with(ws, Some(40), &env, &settings, &project);
        assert_eq!(
            (
                all.max_agent_steps.value,
                all.max_agent_steps.origin.clone()
            ),
            (40, Origin::Flag)
        );
        let no_flag = resolve_with(ws, None, &env, &settings, &project);
        assert_eq!(
            no_flag.max_agent_steps.origin,
            Origin::Env("VIV_MAX_AGENT_STEPS".into())
        );
        let no_env_set = resolve_with(ws, None, &no_env, &settings, &project);
        assert_eq!(
            (
                no_env_set.max_agent_steps.value,
                no_env_set.max_agent_steps.origin.clone()
            ),
            (25, Origin::WorkspaceSettings)
        );
        let top = resolve_with(ws, None, &no_env, &json!({"max_agent_steps": 20}), &project);
        assert_eq!(
            (
                top.max_agent_steps.value,
                top.max_agent_steps.origin.clone()
            ),
            (20, Origin::Settings)
        );
        let repo = resolve_with(ws, None, &no_env, &json!({}), &project);
        assert_eq!(repo.max_agent_steps.value, 10);
        assert!(matches!(repo.max_agent_steps.origin, Origin::Project(_)));
        let none = resolve_with(ws, None, &no_env, &json!({}), &ProjectConfig::default());
        assert_eq!(
            (
                none.max_agent_steps.value,
                none.max_agent_steps.origin.clone()
            ),
            (DEFAULT_MAX_AGENT_STEPS, Origin::Default)
        );
    }

    #[test]
    fn the_result_limit_never_goes_under_the_floor_whatever_sets_it() {
        let low = resolve_with(
            Path::new("x"),
            None,
            &|_| Some("5".to_owned()),
            &json!({}),
            &ProjectConfig::default(),
        );
        assert_eq!(low.max_tool_result_bytes.value, MIN_RESULT_BYTES);
        let listed = list(&low);
        assert_eq!(listed.len(), 2);
        assert!(
            listed.iter().any(|(k, _, o)| *k == "max_tool_result_bytes"
                && o.contains("VIV_MAX_TOOL_RESULT_BYTES"))
        );
    }
}
