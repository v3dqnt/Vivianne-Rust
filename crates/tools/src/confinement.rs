//! The standing file mode of one toolbox, shared by `shell`, `write` and `edit`. The default is `danger-full-access`
//! (no confinement). A confined mode that this machine cannot enforce refuses the command; it never runs it unconfined.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde_json::Value;
use vivianne_sandbox::policy::{Mode, Policy, check_escalation_args};

#[derive(Debug, Clone, Default)]
pub struct Confinement(Arc<AtomicU8>);

impl Confinement {
    pub fn set(&self, mode: Mode) {
        let code = match mode {
            Mode::DangerFullAccess => 0,
            Mode::ReadOnly => 1,
            Mode::WorkspaceWrite => 2,
        };
        self.0.store(code, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get(&self) -> Mode {
        match self.0.load(Ordering::Relaxed) {
            1 => Mode::ReadOnly,
            2 => Mode::WorkspaceWrite,
            _ => Mode::DangerFullAccess,
        }
    }

    #[must_use]
    pub fn is_confined(&self) -> bool {
        self.get() != Mode::DangerFullAccess
    }

    /// The mode one `shell` call asks to run as, when it asks to go wider than the standing one. Such a call needs
    /// the person's approval whatever the approval mode is.
    pub fn asked(&self, name: &str, args: &Value) -> Result<Option<Mode>, String> {
        if name != "shell" {
            return Ok(None);
        }
        let text = |key: &str| args[key].as_str();
        let asked = check_escalation_args(text("sandbox_permissions"), text("justification"))
            .map_err(|e| e.to_string())?;
        let current = self.get();
        match asked {
            Some(mode) if mode == current => Ok(None),
            Some(mode) if current.wider().contains(&mode) => Ok(Some(mode)),
            Some(mode) => Err(format!(
                "sandbox escalation to \"{}\" is not strictly wider than this call's current \"{}\" mode",
                mode.as_str(),
                current.as_str()
            )),
            None => Ok(None),
        }
    }

    /// What the model is told when a mode is standing.
    #[must_use]
    pub fn describe(&self, workspace: &Path) -> Option<String> {
        if !self.is_confined() {
            return None;
        }
        let absolute = std::path::absolute(workspace).ok()?;
        Policy::new(self.get(), &absolute, &[])
            .ok()
            .map(|p| p.describe())
    }
}

/// A configured mode by name. A name that is not a mode means the person meant to restrict: read-only, with why.
#[must_use]
pub fn mode_from_setting(value: Option<&str>) -> (Mode, Option<String>) {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => (Mode::DangerFullAccess, None),
        Some(text) => Mode::parse(text).map_or_else(
            || {
                (
                    Mode::ReadOnly,
                    Some(format!("sandbox: `{text}` is not read-only, workspace-write or danger-full-access; using read-only")),
                )
            },
            |mode| (mode, None),
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_a_strictly_wider_request_with_a_reason_asks_to_escalate() {
        let c = Confinement::default();
        assert_eq!(c.get(), Mode::DangerFullAccess);
        c.set(Mode::ReadOnly);
        let ask = json!({ "command": "x", "sandbox_permissions": "workspace-write", "justification": "save" });
        assert_eq!(c.asked("shell", &ask), Ok(Some(Mode::WorkspaceWrite)));
        assert_eq!(c.asked("read", &ask), Ok(None));
        assert!(
            c.asked(
                "shell",
                &json!({ "sandbox_permissions": "workspace-write" })
            )
            .is_err()
        );
        c.set(Mode::DangerFullAccess);
        assert!(
            c.asked("shell", &ask).is_err(),
            "narrower is not honoured silently"
        );
    }

    #[test]
    fn an_unknown_setting_restricts_rather_than_opens() {
        assert_eq!(mode_from_setting(None), (Mode::DangerFullAccess, None));
        assert_eq!(
            mode_from_setting(Some("workspace-write")).0,
            Mode::WorkspaceWrite
        );
        let (mode, warning) = mode_from_setting(Some("readonly"));
        assert_eq!(mode, Mode::ReadOnly);
        assert!(warning.is_some());
    }
}
