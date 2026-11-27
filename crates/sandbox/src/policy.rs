//! What a command may write: the three modes, the folders `workspace-write` opens, how a model may ask for one wider
//! mode for one call, and what the model is told. Port of the original `sandbox` contract and `sandbox-policy`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl Mode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        [Self::ReadOnly, Self::WorkspaceWrite, Self::DangerFullAccess]
            .into_iter()
            .find(|m| m.as_str() == text)
    }

    /// The modes strictly wider than this one: what one call may be escalated to.
    #[must_use]
    pub fn wider(self) -> &'static [Self] {
        match self {
            Self::ReadOnly => &[Self::WorkspaceWrite, Self::DangerFullAccess],
            Self::WorkspaceWrite => &[Self::DangerFullAccess],
            Self::DangerFullAccess => &[],
        }
    }
}

/// The policy for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub mode: Mode,
    pub workspace: PathBuf,
    /// Further folders `workspace-write` opens.
    pub additional: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("the workspace must be an absolute path: {0}")]
    Workspace(String),
    #[error("invalid additional writable directory {path}: {why}")]
    Root { path: String, why: String },
    #[error("sandbox_permissions requires a justification")]
    MissingJustification,
    #[error("justification is only valid together with sandbox_permissions")]
    StrayJustification,
    #[error("justification must be a non-empty sentence")]
    EmptyJustification,
    #[error("`{0}` is not a sandbox mode a call may ask for")]
    UnknownMode(String),
    #[error(
        "sandbox escalation to \"{asked}\" is not strictly wider than this call's current \"{current}\" mode"
    )]
    NotWider {
        asked: &'static str,
        current: &'static str,
    },
}

fn physical(path: &Path) -> Result<PathBuf, String> {
    let real =
        std::fs::canonicalize(path).map_err(|e| format!("cannot read the directory: {e}"))?;
    if !real.is_dir() {
        return Err("expected an existing directory".to_owned());
    }
    Ok(real)
}

/// The same path ignoring how Windows writes a canonical one (`\\?\`) and trailing separators.
fn comparable(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let text = text.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text.to_owned()
    }
}

impl Policy {
    /// A checked policy. `workspace` must be absolute.
    pub fn new(mode: Mode, workspace: &Path, additional: &[PathBuf]) -> Result<Self, PolicyError> {
        if !workspace.is_absolute() {
            return Err(PolicyError::Workspace(workspace.display().to_string()));
        }
        let mut seen = Vec::new();
        for root in additional {
            if !seen.contains(root) {
                seen.push(root.clone());
            }
        }
        Ok(Self {
            mode,
            workspace: workspace.to_owned(),
            additional: seen,
        })
    }

    /// The folders this mode lets a command write: none unless `workspace-write`. An additional folder must be given by
    /// its real path, so a link cannot be swapped later to open somewhere else.
    pub fn workspace_roots(&self) -> Result<Vec<PathBuf>, PolicyError> {
        if self.mode != Mode::WorkspaceWrite {
            return Ok(Vec::new());
        }
        let mut roots =
            vec![std::fs::canonicalize(&self.workspace).unwrap_or_else(|_| self.workspace.clone())];
        for extra in &self.additional {
            let error = |why: String| PolicyError::Root {
                path: extra.display().to_string(),
                why,
            };
            let real = physical(extra).map_err(error)?;
            if comparable(&real) != comparable(extra) {
                return Err(error(format!(
                    "use the physical directory path {}; symlinked grants are not supported",
                    real.display()
                )));
            }
            if !roots.contains(&real) {
                roots.push(real);
            }
        }
        Ok(roots)
    }

    /// The workspace roots and the temporary areas commands expect to write.
    pub fn writable_roots(&self) -> Result<Vec<PathBuf>, PolicyError> {
        let mut roots = self.workspace_roots()?;
        if self.mode == Mode::WorkspaceWrite {
            for temp in [PathBuf::from("/tmp"), std::env::temp_dir()] {
                let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
                if !roots.contains(&temp) {
                    roots.push(temp);
                }
            }
        }
        Ok(roots)
    }

    /// What the model is told before a request.
    #[must_use]
    pub fn describe(&self) -> String {
        match self.mode {
            Mode::ReadOnly => "Current file policy: read-only. Any available operation enforced by the file sandbox cannot modify files in the standing mode. Do not refuse a required modification from this policy alone: try an available tool normally and follow any denial and escalation guidance it returns.".to_owned(),
            Mode::WorkspaceWrite => {
                let extra = if self.additional.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> = self.additional.iter().map(|p| p.display().to_string()).collect();
                    format!(" Additional writable directories: {}.", serde_json::json!(list))
                };
                format!(
                    "Current file policy: workspace-write. Any available operation enforced by the file sandbox may modify files under the session workspace: {}.{extra} Some platform temporary areas may also be writable.",
                    serde_json::json!(self.workspace.display().to_string())
                )
            }
            Mode::DangerFullAccess => "Current file policy: danger-full-access. The file sandbox does not restrict file modifications by available operations.".to_owned(),
        }
    }
}

/// The standing mode: a session's own switch wins over the deployment's default.
#[must_use]
pub fn standing_mode(default: Mode, session: Option<Mode>) -> Mode {
    session.unwrap_or(default)
}

/// The `sandbox_permissions` and `justification` a call carries, checked together.
pub fn check_escalation_args(
    permissions: Option<&str>,
    justification: Option<&str>,
) -> Result<Option<Mode>, PolicyError> {
    match (permissions, justification) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(PolicyError::MissingJustification),
        (None, Some(_)) => Err(PolicyError::StrayJustification),
        (Some(_), Some(why)) if why.trim().is_empty() => Err(PolicyError::EmptyJustification),
        (Some(mode), Some(_)) => Mode::parse(mode)
            .filter(|m| *m != Mode::ReadOnly)
            .map(Some)
            .ok_or_else(|| PolicyError::UnknownMode(mode.to_owned())),
    }
}

/// What a call may run as once asked: the current mode when it asked for that, a strictly wider one after `approved`
/// says yes (the question carries the reason), otherwise an error. A narrower request is not honoured silently.
pub fn escalate(
    current: Mode,
    asked: Mode,
    justification: &str,
    approved: impl FnOnce(&str) -> bool,
) -> Result<Option<Mode>, PolicyError> {
    if asked == current {
        return Ok(Some(current));
    }
    if !current.wider().contains(&asked) {
        return Err(PolicyError::NotWider {
            asked: asked.as_str(),
            current: current.as_str(),
        });
    }
    Ok(approved(&format!(
        "escalate sandbox to {}: {justification}",
        asked.as_str()
    ))
    .then_some(asked))
}

#[must_use]
pub fn denial_marker(mode: Mode) -> String {
    format!("[sandbox: file access denied under {} mode]", mode.as_str())
}

#[must_use]
pub fn escalation_hint(subject: &str) -> String {
    format!(
        "[sandbox: escalation available — retry this exact {subject} once with sandbox_permissions (the narrowest wider mode that suffices) + justification; the approval prompt asks the user]"
    )
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
