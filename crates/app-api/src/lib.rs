//! Local, versioned API that micro-apps (separate processes) talk to (ADR 0012, 0013).

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to the wire format.
pub const API_VERSION: u32 = 1;

/// What a micro-app may ask for. Declared up front, approved per app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    AgentSessions,
    Tools,
    Memory,
    ModelRouting,
    Panels,
    Network,
    WriteOutsideProject,
}

impl Capability {
    /// Capabilities that need an explicit extra grant even for approved apps.
    #[must_use]
    pub fn is_sensitive(self) -> bool {
        matches!(self, Self::Network | Self::WriteOutsideProject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_snake_case() {
        let s = serde_json::to_string(&Capability::WriteOutsideProject).unwrap();
        assert_eq!(s, r#""write_outside_project""#);
    }

    #[test]
    fn only_network_and_outside_writes_are_sensitive() {
        assert!(Capability::Network.is_sensitive());
        assert!(Capability::WriteOutsideProject.is_sensitive());
        assert!(!Capability::Memory.is_sensitive());
    }
}
