# computer

Computer use: the agent sees and operates desktop apps through `agent-desktop` (vercel-labs, Apache-2.0), run as an MCP server named `computer`.

- `mod.rs`: the on/off setting, the tools the model is offered (`OFFERED`), the server config, `apply()`, and the agent's instructions (`GUIDE`).
- `guard.rs`: what every call goes through: refuses typing secrets, retries once when the program says it is safe, caps result size.
- `install.rs`: finds the program, downloads it from the project's own GitHub release into `~/.viv/bin` after a SHA-256 check, and asks it for its status.

Which tools ask for approval and which results are never saved is decided in `vivianne_core` (`is_look_only`, `returns_desktop_content`); old snapshots are masked in `vivianne_runtime::history`.
