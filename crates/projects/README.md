# vivianne-projects

Shared projects: who can do what, where and why. The roadmap is in `docs/plans/projects.md`. No UI, network or disk.

- **May depend on** (R1.1): `core`.
- **State it owns** (R1.6): the role table, the project, membership, join-request and provider-sync state machines, and the audit event.
- **Not done:** no accounts, no persistent or hosted `Store` (`crates/hub` has the SQLite one), no provider adapter.
- **Check it:** `cargo test -p vivianne-projects` and `cargo clippy -p vivianne-projects --all-targets`.
