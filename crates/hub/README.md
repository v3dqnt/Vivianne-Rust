# vivianne-hub

The hosted Vivianne service for shared projects (`viv-hub`). Roadmap: `docs/plans/projects.md`.

- **May depend on** (R1.1): `projects`. The rules live in `projects`; this crate stores them and serves them.
- **State it owns** (R1.6): the SQLite database (projects, memberships, requests, audit events, session token hashes).
- **Limits:** one node, one database file, no failover. It speaks plain HTTP and expects TLS in front of it.
- **Sign-in is not built.** The operator issues tokens with `viv-hub token issue <user>`.
- **Run it here:** `viv-hub serve --db hub.db` listens on `127.0.0.1:8787`.
- **Check it:** `cargo test -p vivianne-hub`.
