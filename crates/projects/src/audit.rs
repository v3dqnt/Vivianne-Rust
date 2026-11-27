//! Audit events: who did what to which resource. Secrets are taken out when the event is made, so none is ever
//! stored (§10).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{ProjectId, UserId};

const MAX_VALUE: usize = 200;
const SECRET_NAMES: [&str; 6] = [
    "token",
    "secret",
    "password",
    "key",
    "credential",
    "authorization",
];
const SECRET_PREFIXES: [&str; 6] = ["ghp_", "gho_", "ghs_", "github_pat_", "sk-", "Bearer "];
const HIDDEN: &str = "[REDACTED]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User(UserId),
    AgentRun(String),
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub at: u64,
    pub actor: Actor,
    pub action: String,
    pub project: ProjectId,
    pub resource: String,
    pub policy_version: u32,
    /// Ties together the events and jobs of one request.
    pub correlation: String,
    pub details: BTreeMap<String, String>,
}

/// `details` with secrets hidden and long values cut.
#[must_use]
pub fn scrub(details: &[(&str, &str)]) -> BTreeMap<String, String> {
    details
        .iter()
        .map(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            let secret = SECRET_NAMES.iter().any(|s| lower.contains(s))
                || SECRET_PREFIXES.iter().any(|p| value.contains(p));
            let kept = if secret {
                HIDDEN.to_owned()
            } else {
                value.chars().take(MAX_VALUE).collect()
            };
            ((*name).to_owned(), kept)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_hidden_by_name_and_by_shape() {
        let got = scrub(&[
            ("access_token", "abc"),
            ("note", "use ghp_0123456789abcdef"),
            ("role", "admin"),
            ("long", &"x".repeat(999)),
        ]);
        assert_eq!(got["access_token"], HIDDEN);
        assert_eq!(got["note"], HIDDEN);
        assert_eq!(got["role"], "admin");
        assert_eq!(got["long"].len(), MAX_VALUE);
    }
}
