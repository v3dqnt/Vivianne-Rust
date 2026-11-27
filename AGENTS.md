# Agent and contributor conventions

`RULES.md` wins over this file. Cite rule ids (R2.1) in PRs.

## Layout
- `crates/core`: domain types and the lists that decide what asks, what only looks, what is private. No UI, no network.
- `crates/ai`: the AI layer from the old app: providers' API clients, keys, models, paths (`~/.viv`).
- `crates/providers`: the `Provider` trait, the adapters over `ai`, and the ACP coding agents.
- `crates/tools`: the agent's tools (with background jobs and terminals), the MCP client, git, the browser and computer-use wrappers.
- `crates/runtime`: the one agent loop, saved threads, checkpoints, automations, orchestration (subagents, the task board), the ledger.
- `crates/memory`, `crates/multiplayer`, `crates/remote` (machines, and the `viv acp` server), `crates/telemetry`: features on top.
- `crates/cli` (`viv`) and `crates/shell` (`vivianne`, the GPUI app): the two surfaces. They only wire.
- The allowed crate graph is `scripts/check-boundaries.sh`; the diagram is `docs/architecture.md`.

## State and configuration
- Everything lives under `~/.viv` (or `$VIV_HOME`). Threads in `agent/threads/<id>.json`, settings in `agent/settings.json`, machines in `client/machines.json`, tool results too big for the model in `agent/tool-results/`.
- Configuration layers, highest wins: command-line flag, environment variable, `settings.json`, then the project's `<workspace>/.viv.json`, then the built-in default. `.viv.json` is shared with a repository, so it may set only the repo-safe fields (`max_agent_steps`, `max_tool_result_bytes`, `context`); anything else is ignored with a warning, and it can never set permissions (R4.4). `viv config list --show-origin` says where each value came from.
- Agent profiles: a folder with `profile.json` (at most 64 KiB) and an optional `prompt.md` (at most 256 KiB), in `~/.viv/agents/<name>/` or the repository's `.viv/agents/<name>/`. A repository profile cannot set `permission_mode`.

## Adding things
- A tool: its name and spec in `crates/tools/src/schemas.rs`, its code in its own module, its place in the permission lists in `crates/core` (an unlisted tool asks).
- A command, flag, setting or JSON field: `docs/cli-shape.md` (R5.1).
- A new crate edge: `scripts/check-boundaries.sh` and `docs/architecture.md` (R1.1).

## Checks
`cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `bash scripts/check-boundaries.sh`. Build the app in release (`cargo build --release -p vivianne-shell`) before judging how smooth it is: a debug build is not representative.
