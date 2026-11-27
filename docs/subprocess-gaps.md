# The original `subprocess` package group, and where Vivianne stands

The original is three packages (about 6 700 lines of source, 10 400 of tests): the `ctx.subprocess` contract, a local
provider, and Win32 process bindings. Code here: `crates/sandbox` (`env`, `output`, `tree`, `spawn`).

| Original | Vivianne | State |
|---|---|---|
| Spawn with argv, cwd, stdio modes (ignore, pipe, inherit, data, collect), grace, abort signal, env | `spawn::Spec`, `spawn` | **Ported.** |
| Scrubbed environment (credential-shaped names and `DSH_*` removed, explicit entries merged after) | `env::scrubbed`, `child_env` | **Ported,** with Vivianne's wider credential list and the `VIV_` prefix. |
| Executable lookup on the child's `PATH` | `spawn::find_program` | **Ported.** |
| Bounded tail, spill file, incremental reads with a lossy flag | `output::Collector` | **Ported.** |
| Managed process ranges; terminate, wait for exit | `tree::Tree`, `spawn::Handle` | **Ported:** a job object on Windows, a process group on Unix; leftovers are ended when the run ends or the handle is dropped. On Windows the child joins its job just after starting, so the very first instant is not covered. |
| Linux `systemd` scopes (cgroup ranges) and their selector | none | **Not ported.** |
| Process and shell-activity inspectors | none | **Not ported.** |
| Control channel (`stdio.control`) for out-of-process subagents | none | **Not ported:** Vivianne runs subagents in-process. |
| Terminal (PTY) process primitive | none | Belongs to the `terminal` port. |
| Used by the shell tool, language servers, subagent backends | `tools::simple::Shell` | **Wired into `shell`** (so every agent: app, `viv`, remote, subagents), which now also gets the scrubbed environment and ends everything it started when it finishes. LSP and MCP still start processes their own way. |

Tested on Windows only. The Unix paths have not been compiled or run.
