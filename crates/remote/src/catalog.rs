//! The saved machines, kept in `~/.viv/client/machines.json`. Only what is needed to reach a machine: an id, a label,
//! the SSH target, the remote session's name and whether it is switched on. No password, private key or agent
//! ticket is ever stored here; authentication stays with OpenSSH.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CATALOG_VERSION: u32 = 1;
pub const MAX_CATALOG_BYTES: u64 = 64 * 1024;
pub const MAX_PROFILES: usize = 64;
pub const MAX_LABEL_BYTES: usize = 128;
pub const MAX_TARGET_BYTES: usize = 1024;
pub const DEFAULT_SESSION: &str = "default";

/// One machine the user saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedMachine {
    /// 32 lowercase hex characters, opaque.
    pub id: String,
    pub label: String,
    /// An alias from `~/.ssh/config`, `user@host`, or `ssh://user@host:port`.
    pub target: String,
    /// The remote server's session name.
    pub session: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub version: u32,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub ssh: Vec<SavedMachine>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: CATALOG_VERSION,
            selected: None,
            ssh: Vec::new(),
        }
    }
}

#[must_use]
pub fn path() -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["client", "machines.json"])
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

/// Whether `id` is a machine id as this module makes them.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A target is handed to `ssh` as an argument, so it must not look like an option, hold whitespace or control
/// characters, or carry a password.
pub fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty() || target.len() > MAX_TARGET_BYTES {
        return Err(format!(
            "A target is 1 to {MAX_TARGET_BYTES} characters long."
        ));
    }
    if has_control(target) || target.chars().any(char::is_whitespace) {
        return Err("A target has no spaces or control characters.".to_owned());
    }
    if target.starts_with('-') {
        return Err("A target cannot start with a dash.".to_owned());
    }
    let rest = target.strip_prefix("ssh://").unwrap_or(target);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if let Some((userinfo, _)) = authority.rsplit_once('@')
        && userinfo.contains(':')
    {
        return Err(
            "Do not put a password in a target: authentication stays with OpenSSH.".to_owned(),
        );
    }
    Ok(())
}

/// A session name is used in paths and a remote shell command.
pub fn validate_session(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("A session name is 1 to 64 letters, digits, dashes or underscores.".to_owned());
    }
    Ok(())
}

fn validate_label(label: &str) -> Result<(), String> {
    if label.trim().is_empty() || label.len() > MAX_LABEL_BYTES || has_control(label) {
        return Err(format!(
            "A label is 1 to {MAX_LABEL_BYTES} characters, with no control characters."
        ));
    }
    Ok(())
}

/// The host part of a target, for a default label.
fn host_of(target: &str) -> String {
    let rest = target.strip_prefix("ssh://").unwrap_or(target);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    host.split(':').next().unwrap_or(host).to_owned()
}

/// The label a machine gets when none is given: its host, or `host/session` for a session that is not the default.
#[must_use]
pub fn default_label(target: &str, session: &str) -> String {
    let host = host_of(target);
    if session == DEFAULT_SESSION {
        host
    } else {
        format!("{host}/{session}")
    }
}

impl SavedMachine {
    fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err("A machine id is 32 lowercase hex characters.".to_owned());
        }
        validate_label(&self.label)?;
        validate_target(&self.target)?;
        validate_session(&self.session)
    }
}

impl Catalog {
    /// Reads and checks a catalog file. A missing file is an empty catalog.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        let Ok(meta) = std::fs::metadata(path) else {
            return Ok(Self::default());
        };
        if meta.len() > MAX_CATALOG_BYTES {
            return Err("The machines file is too big to be one of ours.".to_owned());
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let catalog: Self = serde_json::from_str(&text)
            .map_err(|e| format!("The machines file could not be read: {e}"))?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        vivianne_ai::paths::write_json_atomic(path, self)
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&path())
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != CATALOG_VERSION {
            return Err(format!(
                "The machines file is version {}, this app reads version {CATALOG_VERSION}.",
                self.version
            ));
        }
        if self.ssh.len() > MAX_PROFILES {
            return Err(format!("At most {MAX_PROFILES} machines can be saved."));
        }
        let mut seen = std::collections::HashSet::new();
        for m in &self.ssh {
            m.validate()?;
            if !seen.insert(m.id.as_str()) {
                return Err("Two machines share an id.".to_owned());
            }
        }
        Ok(())
    }

    /// A machine by its id or its label.
    #[must_use]
    pub fn find(&self, id_or_label: &str) -> Option<&SavedMachine> {
        self.ssh
            .iter()
            .find(|m| m.id == id_or_label)
            .or_else(|| self.ssh.iter().find(|m| m.label == id_or_label))
    }

    fn index(&self, id_or_label: &str) -> Result<usize, String> {
        self.ssh
            .iter()
            .position(|m| m.id == id_or_label)
            .or_else(|| self.ssh.iter().position(|m| m.label == id_or_label))
            .ok_or_else(|| format!("No saved machine is called {id_or_label}."))
    }

    /// Saves a new machine.
    pub fn add(
        &mut self,
        target: &str,
        label: Option<&str>,
        session: Option<&str>,
    ) -> Result<&SavedMachine, String> {
        let session = session.unwrap_or(DEFAULT_SESSION);
        let label = label.map_or_else(|| default_label(target, session), str::to_owned);
        let machine = SavedMachine {
            id: uuid::Uuid::new_v4().simple().to_string(),
            label,
            target: target.to_owned(),
            session: session.to_owned(),
            enabled: true,
        };
        machine.validate()?;
        if self.ssh.iter().any(|m| m.label == machine.label) {
            return Err(format!(
                "A machine is already called {}; pass a different --label.",
                machine.label
            ));
        }
        if self.ssh.len() >= MAX_PROFILES {
            return Err(format!("At most {MAX_PROFILES} machines can be saved."));
        }
        self.ssh.push(machine);
        Ok(&self.ssh[self.ssh.len() - 1])
    }

    pub fn rename(&mut self, id_or_label: &str, label: &str) -> Result<(), String> {
        validate_label(label)?;
        let i = self.index(id_or_label)?;
        if self
            .ssh
            .iter()
            .enumerate()
            .any(|(j, m)| j != i && m.label == label)
        {
            return Err(format!("A machine is already called {label}."));
        }
        self.ssh[i].label = label.to_owned();
        Ok(())
    }

    pub fn set_enabled(&mut self, id_or_label: &str, enabled: bool) -> Result<(), String> {
        let i = self.index(id_or_label)?;
        self.ssh[i].enabled = enabled;
        Ok(())
    }

    /// Forgets a machine. Whatever runs on it keeps running: only this app's link is gone.
    pub fn remove(&mut self, id_or_label: &str) -> Result<SavedMachine, String> {
        let i = self.index(id_or_label)?;
        let gone = self.ssh.remove(i);
        if self.selected.as_deref() == Some(gone.id.as_str()) {
            self.selected = None;
        }
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_an_alias_or_a_url_and_never_an_option_a_password_or_a_command() {
        for fine in ["workbox", "me@host", "ssh://me@host:2222", "10.0.0.5"] {
            assert!(validate_target(fine).is_ok(), "{fine}");
        }
        for bad in [
            "",
            "-oProxyCommand=evil",
            "host name",
            "me:secret@host",
            "ssh://me:secret@host:22",
            "a\nb",
        ] {
            assert!(validate_target(bad).is_err(), "{bad:?}");
        }
        // A colon after the host is a port, not a password.
        assert!(validate_target("me@host:22").is_ok());
    }

    #[test]
    fn labels_default_to_the_host_and_machines_are_added_renamed_disabled_and_removed() {
        let mut c = Catalog::default();
        let id = c
            .add("me@box.example:2222", None, None)
            .expect("adds")
            .id
            .clone();
        assert!(valid_id(&id));
        assert_eq!(c.ssh[0].label, "box.example");
        assert_eq!(default_label("ssh://me@box", "work"), "box/work");
        assert!(
            c.add("me@other", Some("box.example"), None).is_err(),
            "labels are unique"
        );
        c.rename("box.example", "workbox").expect("renames");
        c.set_enabled("workbox", false).expect("disables");
        assert!(!c.find(&id).expect("by id").enabled);
        c.selected = Some(id.clone());
        c.remove("workbox").expect("removes");
        assert!(c.ssh.is_empty() && c.selected.is_none());
        assert!(c.remove("workbox").is_err());
    }

    #[test]
    fn the_file_round_trips_is_checked_and_holds_no_secret_fields() {
        let dir = std::env::temp_dir().join(format!("viv-catalog-{}", uuid::Uuid::new_v4()));
        let file = dir.join("machines.json");
        assert_eq!(
            Catalog::load_from(&file).expect("missing is empty"),
            Catalog::default()
        );
        let mut c = Catalog::default();
        c.add("workbox", None, Some("agents")).expect("adds");
        c.save_to(&file).expect("saves");
        assert_eq!(Catalog::load_from(&file).expect("loads"), c);
        let text = std::fs::read_to_string(&file).expect("reads");
        for secret in ["password", "private_key", "control_socket", "passphrase"] {
            assert!(!text.contains(secret), "{secret}");
        }
        // Unknown fields and a wrong version are refused, not guessed at.
        std::fs::write(&file, r#"{"version":1,"ssh":[],"password":"x"}"#).expect("writes");
        assert!(Catalog::load_from(&file).is_err());
        std::fs::write(&file, r#"{"version":9,"ssh":[]}"#).expect("writes");
        assert!(Catalog::load_from(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
