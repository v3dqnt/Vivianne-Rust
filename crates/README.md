# crates

Dependency direction: `shell` -> `runtime` -> `providers`, `tools`, `plugins` -> `core`.

| Crate | Role |
|---|---|
| `core` | Domain types, no I/O |
| `plugins` | Context, services, events, reversible registrations |
| `providers` | Model provider trait and adapters |
| `tools` | Agent tools (shell, browser, desktop control) |
| `runtime` | The one agent loop |
| `projects` | Shared projects: roles, membership and permission rules |
| `hub` | `viv-hub`, the service that stores and serves them |
| `sandbox` | Starting and confining other programs; the only crate with OS-level `unsafe` |
| `app-api` | API for micro-app processes |
| `cli` | `viv`, headless chat through `runtime` |
| `shell` | GPUI app; only crate that touches GPUI |

Layout of each crate: `Cargo.toml`, `README.md`, `src/` (with its own `README.md`).
