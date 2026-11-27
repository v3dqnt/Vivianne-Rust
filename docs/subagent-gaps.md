# Subagents: where Vivianne stands against the DeepSeek `subagent` packages

Code: `crates/runtime/src/orchestration`. Until this change the tools below existed but **no surface offered them**; the app, `viv`, the remote server and `viv acp` now all do.

## What works

- `task`, `task_output`, `wait_tasks`, `kill_task`, `list_subagent_profiles`; at most 6 at once; subagents cannot start subagents.
- Vivianne's own subagents: a fresh conversation on the thread's model, read-only unless the thread is in Auto mode. One that would have to ask the person is told no.
- `task` with `agent`: a coding agent from the Agent Client Protocol registry does the work (port of `subagent-acp`).

## `subagent-acp`

| Original | State |
|---|---|
| A configured command and arguments | **Narrowed:** only agents in the ACP registry, already set up on this machine. No setting for a command of your own. |
| One child process per run, ended with it | **Ported.** A stopped run gets 10 seconds to end its turn. |
| Permission policy `reject` (default) or `allow` | **Replaced.** Started only when the thread is in Auto mode and no sandbox mode is set; its requests are then allowed. A read-only profile is asked to plan and its requests refused. An agent that acts without asking is not stopped by either. |
| Stop reasons to outcomes; partial text kept | **Ported.** Cut off or refused counts as failed, with what was said. |
| Child environment: scrubbed, explicit entries on top | **Not changed:** the child starts as coding agents already do here. |
| Teardown ladder (EOF, then terminate) | **Not ported:** the process tree is ended when the connection is dropped. |
| Working directory inheritance | The project folder. |
| Safe fixed-form diagnostics | **Not ported.** The agent's own error text is passed on. |

## Not ported from the other `subagent` packages

The `claude-code` and `codex` one-shot providers (reach those agents through `agent`), `dsh-sdk`, fork in process (children seeded with the parent's turns), `tool-subagent-control` (messaging and interrupting a tree), structured output, depth settings, agent presets. The thread tools (`message_thread` and the others) exist but are still not offered: nothing implements their hub.

## Limits to know

- A subagent's usage is recorded under the thread that started it. A run limit is checked when the parent's own usage arrives, so subagents can overshoot it while the parent waits.
- Stopping a turn stops every subagent the thread still has working, including ones started in earlier turns.
- Whether that run reported its usage is not known: usage is buffered, and the test process ended before a flush. A one-shot `viv` can lose a subagent's usage the same way.
- Subagents end with the process; a one-shot `viv` that does not wait loses them.
- A coding agent runs its own tools: Vivianne's sandbox, change records and checkpoints do not cover what it does beyond the checkpoint around the whole turn.

## Tests

Covered: refusal outside Auto mode and under a sandbox mode; a stopped turn stopping its subagent; finished, stopped and cut-off turns with a stand-in agent; Vivianne's ACP client driving `viv acp` in memory (allow writes, reject does not). Checked once by hand: the built `viv` running its own subagent against a stand-in model. Run once with a real coding agent (`claude-acp`, 2026-11-27) through `task`: it read a note, wrote the answer file after its requests were allowed, and replied, in 243 seconds with 2 tool calls; why it took that long was not looked into. Rerun with the ignored test `a_real_coding_agent_does_a_task` (spends quota). No other coding agent has been tried. Not covered: usage recording, a stop during startup, the original's other cases (its tests were not translated).
