# tools/src/dap

Debugging a running program through a debug adapter (DAP).

- `client.rs`: the protocol over an adapter's stdio. `config.rs`: the adapter table (never taken from a project unless it is trusted) and what a launch will run. `session.rs`: one session per workspace folder; breakpoints are sent before `configurationDone`.
- `tools.rs` + `schemas.rs`: the agent's `debug` (looks; allowed in Plan mode) and `debug_run` (starts, steers, ends; asks first, and a launch shows the exact command).
- `ui.rs`: what the Debug tab in the rail reads (`cmd_debug_snapshot`) and its buttons (`cmd_debug_control`).

Copied from the original app and kept close to it, so the module is `rustfmt::skip`.
