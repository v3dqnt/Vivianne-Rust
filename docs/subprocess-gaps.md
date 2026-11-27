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
| Terminal (PTY) process primitive | `tools::terminal` | Ported separately on `alacritty_terminal`; see `terminal-gaps.md`. |
| Used by the shell tool, language servers, subagent backends | `shell`, jobs, language servers, debug adapters | **Wired** into those: scrubbed environment, and the whole tree ends with its owner. MCP servers, coding agents (ACP), one-shot checkers, the dev server and document converters still start processes their own way. |

One list now says what counts as a credential (`env::withheld`); the tools crate's check calls it, so MCP servers no longer receive `VIV_*` variables either.

## Tests

The original's tests (`subprocess` 10, `subprocess-local` 315, `win32-process` 51) were **not** translated. Here: the `env`, `output` and `spawn_tests.rs` tests in `crates/sandbox`, and the debug adapter tests that start a real adapter through it. A real language server (`gopls`) starts, initialises and stops through the new path (an ignored test in `lsp/client.rs`).

Tested on Windows only. The Unix paths have not been compiled or run: no Linux or macOS target is installed and CI runs on Windows.
