# runtime/src/orchestration

Subagents and threads working together: starting a subagent with a profile, collecting its result, and one thread reading or messaging another.

- `subagents.rs`: the registry of running subagents and the driver for Vivianne's own.
- `coding.rs`: the driver for a coding agent over ACP.
- `spawner.rs`: the agent a subagent of Vivianne's own runs as.
- `tools.rs`: `task` and the tools that collect and stop.
- `threads.rs`: thread messaging; not offered yet.
- `profiles.rs`: the kinds of subagent.
- `team/`: the shared task board; see its README.
