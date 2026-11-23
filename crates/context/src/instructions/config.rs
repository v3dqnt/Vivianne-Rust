//! Settings of the instruction loader, resolved once. The home folders are process policy (`VIV_HOME`, `VIV_AGENTS_HOME`),
//! not settings of the loader: instruction loading always reads the process's own homes.

use std::path::{Path, PathBuf};

use super::render::{
    AGENTS_ENV_DISPLAY, DEFAULT_AGENTS_DISPLAY, DEFAULT_HOME_DISPLAY, HOME_ENV_DISPLAY,
};

pub const DEFAULT_MAX_SOURCE_BYTES: u64 = 1_048_576;
/// Each chain's default budget: the bound on what workspace instructions may spend.
pub const DEFAULT_MAX_BYTES: usize = 65_536;
const RESERVED_SEGMENTS: [&str; 3] = ["", ".", ".."];

/// What a person may set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Directory entries that mark the project root while walking up from the working directory.
    pub project_root_markers: Vec<String>,
    /// Bytes each chain (user-global, project) may take in one message, so a message with both is at most twice this.
    /// Zero turns loading off.
    pub max_bytes: usize,
    /// Largest instruction file read; a bigger one is ignored.
    pub max_source_bytes: u64,
    /// File names loaded in each project directory, in order.
    pub instruction_file_candidates: Vec<String>,
    /// Personal overlays loaded after the base files.
    pub local_instruction_file_candidates: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project_root_markers: vec![".git".into()],
            max_bytes: DEFAULT_MAX_BYTES,
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            instruction_file_candidates: vec!["AGENTS.md".into(), "CLAUDE.md".into()],
            local_instruction_file_candidates: vec![
                "AGENTS.local.md".into(),
                "CLAUDE.local.md".into(),
            ],
        }
    }
}

/// The configuration plus the two home folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub config: Config,
    pub viv_home: PathBuf,
    pub viv_home_display: String,
    pub agents_home: PathBuf,
    pub agents_home_display: String,
}

/// A candidate name is one path segment: not empty, not `.` or `..`, and no separator.
fn valid_candidates(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| !RESERVED_SEGMENTS.contains(&n.as_str()) && !n.contains(['/', '\\']))
        .cloned()
        .collect()
}

impl Resolved {
    /// Resolves with explicit homes. `viv_env`/`agents_env` say whether the home came from its environment variable,
    /// which decides how the model sees its path.
    #[must_use]
    pub fn new(
        config: Config,
        viv_home: &Path,
        viv_env: bool,
        agents_home: &Path,
        agents_env: bool,
    ) -> Self {
        let config = Config {
            instruction_file_candidates: valid_candidates(&config.instruction_file_candidates),
            local_instruction_file_candidates: valid_candidates(
                &config.local_instruction_file_candidates,
            ),
            ..config
        };
        Self {
            config,
            viv_home: viv_home.to_path_buf(),
            viv_home_display: if viv_env {
                HOME_ENV_DISPLAY
            } else {
                DEFAULT_HOME_DISPLAY
            }
            .to_owned(),
            agents_home: agents_home.to_path_buf(),
            agents_home_display: if agents_env {
                AGENTS_ENV_DISPLAY
            } else {
                DEFAULT_AGENTS_DISPLAY
            }
            .to_owned(),
        }
    }

    /// From the process environment: `VIV_HOME` (else `~/.viv`) and `VIV_AGENTS_HOME` (else `~/.agents`).
    #[must_use]
    pub fn from_env(config: Config) -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let viv = std::env::var_os("VIV_HOME").map(PathBuf::from);
        let agents = std::env::var_os("VIV_AGENTS_HOME").map(PathBuf::from);
        Self::new(
            config,
            &viv.clone().unwrap_or_else(|| home.join(".viv")),
            viv.is_some(),
            &agents.clone().unwrap_or_else(|| home.join(".agents")),
            agents.is_some(),
        )
    }

    /// Identifies the discovery, precedence and budget of one baseline, so a resumed baseline made under other rules is
    /// replaced instead of trusted.
    #[must_use]
    pub fn baseline_identity(&self, cwd: &Path, project_root: &Path) -> String {
        let relative = project_root.strip_prefix(cwd).ok().map_or_else(
            || {
                cwd.strip_prefix(project_root).map_or_else(
                    |_| project_root.display().to_string(),
                    |up| up.components().map(|_| "..").collect::<Vec<_>>().join("/"),
                )
            },
            |p| p.display().to_string(),
        );
        serde_json::json!({
            "projectRoot": relative,
            "projectRootMarkers": self.config.project_root_markers,
            "maxBytes": self.config.max_bytes,
            "maxSourceBytes": self.config.max_source_bytes,
            "instructionFileCandidates": self.config.instruction_file_candidates,
            "localInstructionFileCandidates": self.config.local_instruction_file_candidates,
        })
        .to_string()
    }

    /// Whether loading is on.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.config.max_bytes > 0 && self.config.max_source_bytes > 0
    }
}
