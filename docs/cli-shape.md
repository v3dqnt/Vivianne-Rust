# The shape of `viv` (R5.1)

Every command, flag, setting, environment variable, file and exit code `viv` exposes. A change to any of them is listed here in the same PR.

## Commands
- `viv [prompt]`: chat here (reads lines from standard input when there is no prompt).
- `viv status [--json]`: version, home, workspace, number of saved threads and machines.
- `viv doctor [--json]`: checks (home writable, machines file, saved conversations readable, `.viv.json`, model and key, profiles, ssh). Exit 1 if one fails.
- `viv config list [--show-origin] [--json]`: the settings with where each came from.
- `viv sessions [--limit N] [--json]`: saved threads, newest first.
- `viv session <id> [--tail N] [--json]`: one saved thread.
- `viv profiles [--json]`: agent profiles (user, repository, built-in).
- `viv thread list [--json]`: this project's saved conversations.
- `viv thread events <id> [--from N] [--to M] [--json]`: one conversation as numbered, typed events (user, context, assistant, tool-result, checkpoint) with their turns.
- `viv thread stats <id> [--json]`: turns, steps, tool calls, failures, context messages, model and tool time of one conversation.
- `viv thread outline <id> [--json]`: every turn with a one-line prompt and a short preview of its answer.
- `viv thread fork <id> --before <turn> [--json]`: a new conversation made of the earlier turns of `<id>` (the original is untouched); prints the new id.
- `viv thread export <id> [--out FILE] [--after N] [--raw]`: the conversation as a session log, one JSON object per line (a `session` header, then each message with its position). Secrets and the home folder's name are taken out unless `--raw`; `--after N` skips the messages before position N so a later export continues an earlier one. Writes to standard output without `--out`; nothing is sent anywhere.
- `viv thread generations <id> [--json]`: the earlier states of a conversation that were kept when it was condensed or rewound.
- `viv thread mention <id>`: the text that mentions a conversation in a message (`@[title](viv-session:...)`).
- `viv machine add <target> [--label L] [--remote-session S]`, `list`, `status`, `reconnect <m>`, `rename <m> <label>`, `enable <m>`, `disable <m>`, `remove <m>`, `install <m> [--yes]` (all take `--json` where they print).
- `viv --machine <m> --workspace <dir> [--auto|--plan] <prompt>`: run a prompt on a machine.
- `viv [--session S] server run|start|stop|status|sessions|delete <name>`: this machine's server.
- `viv [--session S] remote-client-bridge [--check] [--idle-timeout-v1]`: run by `ssh` on the machine.

## Global flags
- `--session S`: the server session (default `default`, or `$VIV_SESSION`).

## Exit codes
0 success; 1 failure (a failed check, a failed run); 2 usage error.

## Environment
`VIV_RUN_CODE_STATE` (`persistent` keeps a REPL environment for `run_code` programs within a conversation; anything else, or unset, gives each program a fresh one), `VIV_SOFFICE` (the LibreOffice executable used to convert Office files to PDF; otherwise `PATH` and the usual install places are searched), `VIV_AGENTS_HOME` (replaces `~/.agents`, the shared agents folder whose `AGENTS.md` is read as the user's instructions), `VIV_HOME` (replaces `~/.viv`; must be an absolute folder that is not a root or the home folder), `VIV_SESSION`, `VIV_SSH` (the ssh program), `VIV_MODEL`, `VIV_PROVIDER`, `VIV_MAX_AGENT_STEPS`, `VIV_MAX_TOOL_RESULT_BYTES`, `VIV_REMOTE_BINARY` (a file to install on a machine), `VIV_RELEASE_BASE` (an https folder with `viv-<os>-<arch>` and its `.sha256`).

## Files
`~/.viv/AGENTS.md` and `~/.agents/AGENTS.md` (the user's own instructions; `AGENTS.local.md` and `CLAUDE.md` / `CLAUDE.local.md` beside any `AGENTS.md` in the project or here are read too), `~/.viv/agent/settings.json`, `~/.viv/agent/threads/<id>.jsonl` (the conversation as an append-only log: a header line with `"version"`, then one line per message and per settings change; a log from a newer Vivianne is left untouched and reported by `viv doctor`; a half line at the end is ignored), `~/.viv/agent/threads/<id>.g<N>.jsonl.gz` (an earlier state of the conversation, kept when it was condensed or rewound), `~/.viv/agent/threads/<id>.proj.json` (saved figures and outline, rebuilt when they do not match the conversation), `~/.viv/agent/threads/<id>.v<N>.bak` (the original of a thread an older Vivianne saved as one `<id>.json`, kept when it was carried forward from format N), `~/.viv/agent/threads/<id>.lock` (held by whoever is writing that conversation), `~/.viv/agent/tool-results/<id>.txt`, `~/.viv/client/machines.json`, `~/.viv/client/machine-metadata.json`, `~/.viv/client/ssh/`, `~/.viv/server/sessions/<name>/` (`session.json`, `session-snapshots/`, `session-backups/`), `~/.viv/agents/<name>/` and `<repo>/.viv/agents/<name>/` (`profile.json`, `prompt.md`), `<repo>/.viv.json` (repo-safe settings only), `~/.viv/agent-detection/<agent>.toml`.

## Settings
`max_agent_steps` (default 100), `max_tool_result_bytes` (default 65536, at least 1024); layers: flag, environment, `settings.json` (workspace then top level), `.viv.json`, default.

## Saved message fields
A saved message may carry `context` (`{"kind": "agent-instructions" | "time-context" | "tmux-context" | "session-reference", ...}`: the app added it for the model, so it is not shown in the transcript or counted as something the person typed) and `at` (milliseconds since the epoch when it was added). Both are optional; an unknown `kind` reads as absent, so an older build still opens the thread.

## Message syntax
Sending a message while the agent is working steers the run: the agent reads it before its next model call, and a reply that was about to end goes on to answer it. A coding agent that runs its own loop cannot be steered, and says so.
`@path` and `@"path with spaces"` name a file or folder in the prompt; `@[label](viv-session:<id>)` names another saved conversation (at most 3 per message), whose typed messages and replies are quoted to the model as an untrusted snapshot.

## Build profiles
`dev` (line tables only, no debug info for dependencies), `debugging` (`dev` with full debug info: `cargo build --profile debugging`), `release` (thin LTO, one codegen unit, debug info stripped).

## Tools
`run_code` (runs a JavaScript program that can call only the read-only tools: `read`, `ls`, `grep`, `code_search`, `glob`, as async functions; argument `code`; limits 60 s, 64 MiB, 50 calls, 64 KiB of output; look-only, so it needs no approval and works in Plan mode). With `VIV_RUN_CODE_STATE=persistent` its programs are REPL programs and top-level variables survive from one call to the next within a conversation.

`working_directory` (reads the directory relative paths and commands start from, or moves it with `path`, to an existing directory inside the project; it can never leave the project, and a move is replayed from the saved conversation when it is resumed; look-only, so it needs no approval and works in Plan mode).

`convert_to_pdf` (converts a .doc/.docx/.xls/.xlsx/.ppt/.pptx in the project to a PDF with an installed LibreOffice; arguments `path`, `output?`, `overwrite?`; writes a file, so it asks in Ask mode and is refused in Plan mode; offered only when LibreOffice is found).

`present` (declares up to 8 existing files as final deliverables; arguments `files: [{path, description?}]`; result lines `Presented <absolute path>`; look-only, so it needs no approval and works in Plan mode).

## Change-recording settings
`max_changed_files` (500), `max_changed_file_bytes` (2097152), `change_diff_timeout_ms` (100), `change_git_timeout_ms` (30000), `change_git_output_max_bytes` (8388608). Environment: `VIV_MAX_CHANGED_FILES`, `VIV_MAX_CHANGED_FILE_BYTES`, `VIV_CHANGE_DIFF_TIMEOUT_MS`, `VIV_CHANGE_GIT_TIMEOUT_MS`, `VIV_CHANGE_GIT_OUTPUT_MAX_BYTES`. Layers: environment, `settings.json` (workspace then top level), default; `.viv.json` cannot set them (R4.4). A value of 0 is raised to 1. Listed by `viv config list --show-origin`.

## viv-hub (the shared-projects service)

Commands (a usage error exits 2, a failure 1):

- `viv-hub serve [--db FILE] [--addr HOST:PORT] [--tls-in-front]`: serves until Ctrl+C. Defaults: `viv-hub.db` here, `127.0.0.1:8787`. It speaks plain HTTP and refuses a non-loopback address without `--tls-in-front`.
- `viv-hub token issue <user> [--db FILE] [--days N]`: prints a new token once (30 days by default, at most 365). `<user>` is letters, digits, `-`, `_`, `.`, at most 64. This is the operator's bootstrap; sign-in is not built.
- `viv-hub token revoke <user> [--db FILE]`: removes all of a person's tokens.

State: one SQLite file (`user_version` 1; a newer one is refused and left as it is), with its `-wal` and `-shm` files. No environment variables or settings.

HTTP, JSON in and out. Every call but the directory needs `Authorization: Bearer <token>`. `Idempotency-Key` (1 to 64 letters, digits, hyphens) makes a repeated create or join request return the first result. Bodies are at most 64 KiB. Errors are `{"error": text}` with 400, 401, 403, 404, 408, 409, 413, 429, 500 or 503; a project the caller may not see answers like a missing one.

| Call | Body |
|---|---|
| `GET /v1/directory?after=SLUG` | |
| `GET /v1/me` | |
| `POST /v1/projects` | `slug`, `name`, `summary?`, `visibility?`, `join_policy?` |
| `GET /v1/projects/{id}` | |
| `PUT /v1/projects/{id}/visibility` | `visibility` |
| `PUT /v1/projects/{id}/lifecycle` | `lifecycle` |
| `GET /v1/projects/{id}/members?after=USER` | |
| `PUT /v1/projects/{id}/members/{user}/role` | `role` |
| `PUT /v1/projects/{id}/members/{user}/status` | `status`, `block?` |
| `POST /v1/projects/{id}/invitations` | `user`, `role` |
| `POST /v1/projects/{id}/invitations/accept` | |
| `POST /v1/projects/{id}/join-requests` | `note?` |
| `GET /v1/projects/{id}/join-requests` | |
| `POST /v1/join-requests/{id}/decide` | `revision`, `approve_as?` (absent rejects) |
| `POST /v1/join-requests/{id}/withdraw` | |
| `POST /v1/projects/{id}/owner/offer` | `user`, `confirmed` |
| `POST /v1/projects/{id}/owner/accept` | |
| `GET /v1/projects/{id}/audit` | |

Values are snake case: visibility `public`, `private`; join policy `open`, `approval`, `invite_only`, `closed`; lifecycle `draft`, `active`, `archived`, `deletion_pending`; role `viewer`, `contributor`, `maintainer`, `admin`, `owner`; status `active`, `suspended`, `removed`.
