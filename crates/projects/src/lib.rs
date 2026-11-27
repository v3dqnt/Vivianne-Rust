//! Shared projects (Phase 0 and the rules of Phase 1 in `docs/plans/projects.md`). Everything here is pure: the
//! clock and ids are passed in, and storage is behind [`store::Store`].

pub mod agent;
pub mod audit;
pub mod join;
pub mod membership;
pub mod memory;
pub mod model;
pub mod permission;
pub mod provider;
#[cfg(any(test, feature = "scenarios"))]
pub mod scenarios;
pub mod service;
mod service_members;
mod service_reads;
pub mod store;
pub mod sync;

pub use model::{Lifecycle, Project, ProjectId, UserId, Visibility};
pub use permission::{Denied, Permission, Role};

#[cfg(test)]
mod tests {
    #[test]
    fn the_in_memory_store_passes_the_scenarios() {
        crate::scenarios::all(crate::memory::MemoryStore::default);
    }
}
