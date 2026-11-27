# The DeepSeek `team` packages, and where Vivianne stands

`team/agent-team` and `team/tool-agent-team`. Here: `crates/runtime/src/orchestration/team`. Off unless the `team` setting is on (`VIV_TEAM=1`, or `"team": true` in `settings.json`).

| Original | State |
|---|---|
| Task board: revisions, `claim`, `release`, `edit`, `set_dependencies`, `complete`, `reopen`, `reassign`, `delete` | **Ported**, with the same error codes. |
| Dependencies, cycles, deleted numbers never reused, 256 live tasks, text limits, write-scope warnings | **Ported.** |
| `team_task_create`, `list`, `get`, `update` with filters and pages | **Ported.** They never ask and work in Plan mode, so a read-only subagent can claim its task. They change no project file. |
| Team guidance | **Rewritten** for this app's tools (`task`, `task_output`, `wait_tasks`). |
| Kept in the lead's session log | **Replaced:** one file per conversation, `~/.viv/agent/teams/<thread>.json`, under a file lock. |
| Mode switched per session from a menu; saved per session | **Not ported:** one setting, read when the agent is built. No switch in the app. |
| Teammates that continue, receive messages and nest | **Not ported.** Subagents here run once, cannot message each other and cannot start more. The lead can hand a task only to itself or a subagent still working. |
| Coding agents as members | **Not possible:** they get none of Vivianne's tools. |
| A board view in the UI, a projection for clients | **Not ported.** |
| `maxTasks` setting | **Not ported:** 256. |

## Limits to know

- A subagent that ends or is stopped keeps its tasks in progress until the lead releases them.
- Subagent names start again at `agent-1` after a restart. Ownership is by id, so an old task is not theirs, but a list filtered by name includes it.
- A one-shot `viv` gets a new conversation, and so a new board, every time. Old boards are not cleared away.

## Tests

Covered: the rules and error codes above in part, the store's reload, numbering and refusal to overwrite a file it cannot read, a read-only subagent claiming and completing a task in Ask mode, and, once by hand, the built `viv` doing the same against a stand-in model. The original's tests (about 2 900 lines) were not translated; not covered: the task limit, pages and filters beyond owner, two processes writing at once. Never run with a real model.
