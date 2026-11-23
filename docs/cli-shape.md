# The shape of `viv` (R5.1)

Every command, flag, setting, environment variable, file and exit code `viv` exposes. A change to any of them is listed here in the same PR.

## Commands
- `viv [prompt]`: chat here (reads lines from standard input when there is no prompt).
- `viv status [--json]`: version, home, workspace, number of saved threads and machines.
- `viv doctor [--json]`: checks (home writable, machines file, `.viv.json`, model and key, profiles, ssh). Exit 1 if one fails.
- `viv config list [--show-origin] [--json]`: the settings with where each came from.
- `viv sessions [--limit N] [--json]`: saved threads, newest first.
- `viv session <id> [--tail N] [--json]`: one saved thread.
- `viv profiles [--json]`: agent profiles (user, repository, built-in).
- `viv machine add <target> [--label L] [--remote-session S]`, `list`, `status`, `reconnect <m>`, `rename <m> <label>`, `enable <m>`, `disable <m>`, `remove <m>`, `install <m> [--yes]` (all take `--json` where they print).
- `viv --machine <m> --workspace <dir> [--auto|--plan] <prompt>`: run a prompt on a machine.
- `viv [--session S] server run|start|stop|status|sessions|delete <name>`: this machine's server.
- `viv [--session S] remote-client-bridge [--check] [--idle-timeout-v1]`: run by `ssh` on the machine.

## Global flags
- `--session S`: the server session (default `default`, or `$VIV_SESSION`).

## Exit codes
0 success; 1 failure (a failed check, a failed run); 2 usage error.

## Environment
`VIV_AGENTS_HOME` (replaces `~/.agents`, the shared agents folder whose `AGENTS.md` is read as the user's instructions), `VIV_HOME` (replaces `~/.viv`; must be an absolute folder that is not a root or the home folder), `VIV_SESSION`, `VIV_SSH` (the ssh program), `VIV_MODEL`, `VIV_PROVIDER`, `VIV_MAX_AGENT_STEPS`, `VIV_MAX_TOOL_RESULT_BYTES`, `VIV_REMOTE_BINARY` (a file to install on a machine), `VIV_RELEASE_BASE` (an https folder with `viv-<os>-<arch>` and its `.sha256`).

## Files
`~/.viv/AGENTS.md` and `~/.agents/AGENTS.md` (the user's own instructions; `AGENTS.local.md` and `CLAUDE.md` / `CLAUDE.local.md` beside any `AGENTS.md` in the project or here are read too), `~/.viv/agent/settings.json`, `~/.viv/agent/threads/<id>.json`, `~/.viv/agent/tool-results/<id>.txt`, `~/.viv/client/machines.json`, `~/.viv/client/machine-metadata.json`, `~/.viv/client/ssh/`, `~/.viv/server/sessions/<name>/` (`session.json`, `session-snapshots/`, `session-backups/`), `~/.viv/agents/<name>/` and `<repo>/.viv/agents/<name>/` (`profile.json`, `prompt.md`), `<repo>/.viv.json` (repo-safe settings only), `~/.viv/agent-detection/<agent>.toml`.

## Settings
`max_agent_steps` (default 100), `max_tool_result_bytes` (default 65536, at least 1024); layers: flag, environment, `settings.json` (workspace then top level), `.viv.json`, default.

## Saved message fields
A saved message may carry `context` (`{"kind": "agent-instructions" | "time-context" | "tmux-context" | "session-reference", ...}`: the app added it for the model, so it is not shown in the transcript or counted as something the person typed) and `at` (milliseconds since the epoch when it was added). Both are optional; an unknown `kind` reads as absent, so an older build still opens the thread.

## Message syntax
`@path` and `@"path with spaces"` name a file or folder in the prompt; `@[label](viv-session:<id>)` names another saved conversation (at most 3 per message), whose typed messages and replies are quoted to the model as an untrusted snapshot.

## Build profiles
`dev` (line tables only, no debug info for dependencies), `debugging` (`dev` with full debug info: `cargo build --profile debugging`), `release` (thin LTO, one codegen unit, debug info stripped).

## Tools
`present` (declares up to 8 existing files as final deliverables; arguments `files: [{path, description?}]`; result lines `Presented <absolute path>`; look-only, so it needs no approval and works in Plan mode).

## Change-recording settings
`max_changed_files` (500), `max_changed_file_bytes` (2097152), `change_diff_timeout_ms` (100), `change_git_timeout_ms` (30000), `change_git_output_max_bytes` (8388608). Environment: `VIV_MAX_CHANGED_FILES`, `VIV_MAX_CHANGED_FILE_BYTES`, `VIV_CHANGE_DIFF_TIMEOUT_MS`, `VIV_CHANGE_GIT_TIMEOUT_MS`, `VIV_CHANGE_GIT_OUTPUT_MAX_BYTES`. Layers: environment, `settings.json` (workspace then top level), default; `.viv.json` cannot set them (R4.4). A value of 0 is raised to 1. Listed by `viv config list --show-origin`.
