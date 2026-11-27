# runtime/src/orchestration/team

A task board shared by a conversation's agent and its subagents, off unless the `team` setting is on.

- `board.rs`: the rules: revisions, actions, dependencies, write scopes, limits.
- `store.rs`: one file per conversation under `~/.viv/agent/teams`.
- `tools.rs`: the four `team_task_*` tools.
- `mod.rs`: the policy text the agents are told.
- `tests.rs`: the rules, the store, and a read-only subagent working the board.
