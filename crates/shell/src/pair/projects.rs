//! Which projects are paired, and with whom, as the connection asks.

use std::path::Path;

use vivianne_multiplayer::{Paired, Projects};
use vivianne_runtime::projects::{self, Project};

/// The paired projects as `~/.viv/agent/projects.json` has them.
#[derive(Debug, Default)]
pub struct PairedProjects;

fn paired(project: Project) -> Option<Paired> {
    let pairing = project.pairing()?;
    Some(Paired {
        id: project.id,
        folder: project.path?.into(),
        peer_id: pairing.peer_id,
        peer_display_name: pairing.peer_display_name,
        git_remote: pairing.git_remote,
    })
}

impl Projects for PairedProjects {
    fn for_peer(&self, peer_id: &str) -> Option<Paired> {
        paired(projects::for_peer(peer_id)?)
    }

    fn for_folder(&self, folder: &Path) -> Option<Paired> {
        paired(projects::paired_at(folder)?)
    }
}
