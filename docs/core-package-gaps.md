# The original `core` package group, and where Vivianne stands

The original `packages/core` is the spine of the harness: ten packages (`scope`, `session`, `system-prompt`, `tools`,
`agent`, `agent-loop`, `agent-default-model`, `agent-tool-presentation`, `tool-executor-native`, `tool-executor-ptc`), about
15 000 lines of source and 35 000 of tests. Vivianne has no plugin container: the same jobs are done by `core` (the types),
`runtime` (the loop), `tools` (the toolbox) and `context` (what is added to a conversation). A 1:1 port of the container
(Cordis scopes, effects, Remote methods) would be a second architecture, so this file records, package by package, what maps,
what was ported now, and what is a deliberate difference or still open.

| Original | Vivianne | State |
|---|---|---|
| `tools` validation (`validateJsonSchemaValue`, `ToolArgsError`) | `tools::validate`, called by `Agent::run_tool` | **Ported.** A call is checked against the tool's parameter schema before approval or execution; the model gets `invalid arguments: …`. Deviation: an optional property written as `null` reads as left out. MCP tools are left to their server. |
| `tools` batch (`executeToolBatch`): parallel-safe calls overlap, exclusive calls are barriers, results in model order | `Agent::run_tools`, `core::is_parallel_safe` | **Ported.** Runs of read-only tools overlap (at most 10, the original's default); anything else runs alone after what precedes it. Answers are added in the model's order. A stopped run answers the rest as cancelled. |
| `tools` pipeline: allow/deny/ask policy, `tools/pre-execute`, guards | `Mode::allows`, `needs_approval`, the permission lists in `core` | Same job, different shape (a mode and lists, not an event pipeline). |
| `tools` per-tool `timeoutMs`, `tools/post-execute` result replacement | `Tool::timeout`, `Agent::with_tool_timeout`, `Agent::with_result_filter` | **Ported.** A call that outlasts its tool's limit (or the agent's default) is stopped and answered with an error; a filter may rewrite any result before the model sees it. No tool has a limit set yet except by the agent default; shell keeps its own. |
| `tools` conditional tools (`register(..., { when })`), `restrict`, `ResolvedToolSet` | `Toolbox::add_when`, `StepInfo`, `Toolbox::is_active` | **Ported.** A condition sees the provider and model of the step; a tool whose condition fails is neither offered nor callable. `restrict` is the mode filter. |
| `tools` presentation descriptors (`presentCall`, `presentResult`) | the shell derives cards from tool name and arguments | Same job; no change. |
| `session` event log: typed events with seq and turn, ranges, fork, repair | `runtime::session` (`events`, `range`, `fork_before`), `store::fork`, `history::repair` | **Ported** as a view over a thread's messages, so it can never disagree with them. Fork cuts before a turn and repairs. Different: compaction still replaces the head instead of hiding it, and a thread is not an append-only file. No UI for forking yet. |
| `system-prompt` ordered sections, agent-scoped overrides | `runtime::sections`, `Agent::set_section` / `set_dynamic_section` / `remove_section` | **Ported.** Named sections with an order (equal orders keep the order added), fixed or dynamic text, replacement by name including the built-in `system`, `context` and `mode`. Not ported: reusable `${variable}` substitution. |
| `agent` handle: steering and follow-up input, `inject context`, idle wait | `Agent::steer`, `workspace_context`, the composer queues the next message | **Ported** at the agent: a message sent during a run is added before the next model call, and one that arrives as the model finishes gets one more step. Open: the composer still queues instead of steering. |
| `agent-loop` turn and step lifecycle, runtime context, retries | `Agent::run_loop`, `resilience/`, `workspace_context.rs`, `context` crate | Ported earlier (loop, retry, fallback, context messages). |
| `agent-default-model` | settings in `~/.viv/agent/settings.json`, the model picker | Same job. |
| `agent-tool-presentation`, `tool-executor-native` | one native executor | Native is what Vivianne does. |
| `tool-executor-ptc` (`run_code` programs over a generated SDK) | `tools::run_code` on `tools::ptc` | **Ported, narrowed.** See `docs/ptc-runtime-gaps.md` for the runtime it runs on. The model writes JavaScript that calls the read-only tools (`read`, `ls`, `grep`, `code_search`, `glob`) as async functions (`await`, `Promise.all`); failures reject with a `ToolError` naming the tool; the result must be lossless JSON. 60 s, 64 MiB, 1 MiB stack, 50 calls, 64 KiB of output; look-only, so no approval. Different: JavaScript only (no Python), read-only tools only, and it sits next to the normal tools instead of replacing them. |
| `scope` (scoped registration, nearest definition wins, disposal) | none | Not needed without a plugin container. |
