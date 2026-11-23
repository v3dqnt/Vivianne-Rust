# vivianne-app-api

The local, versioned API that micro-app processes will talk to: `API_VERSION` and the `Capability` set an app declares and a person approves (ADR 0012, 0013).

Only the version and the capability list exist so far. The server, the panel schema and the permission UI are planned (phase 6 in `docs/architecture.md`), and no crate depends on this one yet.

Rules that will apply when it is built: a capability is granted per app and never widened silently (R4.3), and a new capability or field is listed in `docs/cli-shape.md` (R5.1).

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): no other workspace crate.
- **State it owns** (R1.6): none yet; no crate depends on it.
- **Check it:** `cargo test -p vivianne-app-api` and `cargo clippy -p vivianne-app-api --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
