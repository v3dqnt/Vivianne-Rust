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
`VIV_HOME` (replaces `~/.viv`; must be an absolute folder that is not a root or the home folder), `VIV_SESSION`, `VIV_SSH` (the ssh program), `VIV_MODEL`, `VIV_PROVIDER`, `VIV_MAX_AGENT_STEPS`, `VIV_MAX_TOOL_RESULT_BYTES`, `VIV_REMOTE_BINARY` (a file to install on a machine), `VIV_RELEASE_BASE` (an https folder with `viv-<os>-<arch>` and its `.sha256`).

## Files
`~/.viv/agent/settings.json`, `~/.viv/agent/threads/<id>.json`, `~/.viv/agent/tool-results/<id>.txt`, `~/.viv/client/machines.json`, `~/.viv/client/machine-metadata.json`, `~/.viv/client/ssh/`, `~/.viv/server/sessions/<name>/` (`session.json`, `session-snapshots/`, `session-backups/`), `~/.viv/agents/<name>/` and `<repo>/.viv/agents/<name>/` (`profile.json`, `prompt.md`), `<repo>/.viv.json` (repo-safe settings only), `~/.viv/agent-detection/<agent>.toml`.

## Settings
`max_agent_steps` (default 100), `max_tool_result_bytes` (default 65536, at least 1024); layers: flag, environment, `settings.json` (workspace then top level), `.viv.json`, default.
