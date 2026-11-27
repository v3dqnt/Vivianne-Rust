//! The environment every child starts from: this process's, without anything that looks like a credential and without
//! Vivianne's own `VIV_*` settings. A value a caller means a child to have is passed explicitly and is added after.

use std::collections::BTreeMap;
use std::ffi::OsString;

/// Words that mark a variable as a credential wherever they appear in its name.
const SECRET_WORDS: [&str; 9] = [
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "TOKEN",
    "CREDENTIAL",
    "API_KEY",
    "APIKEY",
    "ACCESS_KEY",
    "PRIVATE_KEY",
];
/// Handles to credential agents.
const SECRET_NAMES: [&str; 2] = ["SSH_AUTH_SOCK", "GPG_AGENT_INFO"];
const OWN_PREFIX: &str = "VIV_";

/// Whether a variable of this name is kept from children. Names are compared without case, as Windows compares them.
#[must_use]
pub fn withheld(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with(OWN_PREFIX)
        || upper.ends_with("_KEY")
        || SECRET_NAMES.contains(&upper.as_str())
        || SECRET_WORDS.iter().any(|w| upper.contains(w))
}

/// This process's environment with the withheld names removed.
#[must_use]
pub fn scrubbed() -> BTreeMap<OsString, OsString> {
    std::env::vars_os()
        .filter(|(name, _)| !withheld(&name.to_string_lossy()))
        .collect()
}

/// The child environment: the scrubbed base, then the caller's entries. `None` removes a variable.
#[must_use]
pub fn child_env(extra: &BTreeMap<OsString, Option<OsString>>) -> BTreeMap<OsString, OsString> {
    let mut env = scrubbed();
    for (name, value) in extra {
        // On Windows `Path` and `PATH` are one variable: drop every spelling before setting one.
        if cfg!(windows) {
            let wanted = name.to_string_lossy().to_ascii_uppercase();
            env.retain(|k, _| k.to_string_lossy().to_ascii_uppercase() != wanted);
        }
        match value {
            Some(value) => {
                env.insert(name.clone(), value.clone());
            }
            None => {
                env.remove(name);
            }
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_own_settings_are_withheld_and_ordinary_names_kept() {
        for name in [
            "OPENAI_API_KEY",
            "aws_access_key_id",
            "GITHUB_TOKEN",
            "DB_PASSWORD",
            "my_secret_thing",
            "SSH_AUTH_SOCK",
            "SIGNING_KEY",
            "VIV_HOME",
            "viv_model",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ] {
            assert!(withheld(name), "{name}");
        }
        for name in [
            "PATH",
            "HOME",
            "LANG",
            "HTTPS_PROXY",
            "KEYBOARD_LAYOUT",
            "MONKEY",
        ] {
            assert!(!withheld(name), "{name}");
        }
    }

    #[test]
    fn a_callers_entries_are_added_after_the_scrub_and_none_removes() {
        let mut extra = BTreeMap::new();
        extra.insert(
            OsString::from("DEPLOY_TOKEN"),
            Some(OsString::from("meant")),
        );
        extra.insert(OsString::from("PATH"), None);
        let env = child_env(&extra);
        assert_eq!(
            env.get(&OsString::from("DEPLOY_TOKEN")),
            Some(&OsString::from("meant"))
        );
        assert!(
            !env.keys()
                .any(|k| k.to_string_lossy().eq_ignore_ascii_case("path"))
        );
    }
}
