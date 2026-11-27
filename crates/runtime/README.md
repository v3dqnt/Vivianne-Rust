# vivianne-runtime

The single agent loop for chats and (later) 24/7 agents. `Agent::run` streams the model, runs the
tools it asks for, feeds results back, and stops when the model answers without tools or hits the
100-step ceiling. Callers watch `AgentEvent`s on a channel; there are no UI or Tauri types here.

- `agent.rs`: the loop. Retries, fallback models, the mode (`set_mode`: Plan offers only tools that look, Ask
  emits `AgentEvent::Approval` and waits for `Agent::answer`, Auto runs; the default here is Auto since a library
  caller has nobody to ask) and condensing the conversation when it fills the window.
- `history.rs`: keeps the history fit to send: every call answered (`repair`), old bulky tool output masked in
  batches (`mask_old_results`), and balanced cuts for condensing (`pick_cut`).
- `compact.rs`: the checkpoint summary (fixed sections, an earlier checkpoint merged forward, discarded if not
  smaller). Starts at 80% of the window (`set_context_window`) or when a provider says a request was too large.
- `attempts.rs`: remembers failing calls so a retry-with-small-variations is shown what already failed.
- `context.rs`: splits a request into system prompt, tool definitions, you, assistant and tool results with one
  token estimate (four characters), so the parts add up.
- `workspace_context.rs`: what is added to a conversation before each model call: the AGENTS.md / CLAUDE.md files (rules in `vivianne-context`), the clock and the tmux pane, as durable messages; keeps the single-line `@import` of an instruction file.
- `changes/`: what each run changed, per file with line counts, and each file's comparison (port of `workspace-changes`): private git snapshots at the start and end of a run, plus whole-file copies before each file-tool edit for what git does not cover. Separate from `checkpoints.rs`, which keeps hidden refs for rolling back.
- `formats.rs`: the saved-thread format as a chain of versions (classify, carry forward, refuse newer). `lease.rs`: one writer per conversation, as an operating-system file lock. `log.rs`: a saved thread as an append-only log (`<id>.jsonl`), with kept generations (`<id>.g<N>.jsonl.gz`). `session_stats.rs`: whole-conversation figures (including first-token and decode times) and the turn outline, as folds. `projection.rs`: the registry of those folds and their cache beside the thread. `export.rs`: a conversation as a redacted session log file. `quotes.rs`: quoting a message as Markdown. `query/`: list, filter, trace and full-text search of saved conversations. `surface.rs`: the tools and sandbox mode every surface adds. `schedule/`: scheduled tasks. `export_bundle.rs` and `archive.rs`: a Markdown transcript and a ZIP of a conversation. The gaps against the original `session` group are in `docs/session-package-gaps.md`.
- `thread_refs.rs`: mentioning another conversation of the project (`@[title](viv-session:...)`): the mention becomes plain text and a hidden, untrusted snapshot of that conversation follows the message.
- `sections.rs`: the system prompt as named, ordered sections. `session.rs`: a thread as typed numbered events, and forking it before a turn (`store::fork`).
- `agent.rs` also checks every call's arguments against the tool's schema (`tools::validate`) and overlaps runs of read-only tools (`core::is_parallel_safe`). The gaps against the original `core` group are in `docs/core-package-gaps.md`.
- `deliverables.rs`: the files the model declared with the `present` tool, read back from the saved history.
- `store.rs`: threads kept on disk (`~/.viv/agent/threads/<id>.json`), listed per project folder.
- `title.rs`: a thread's title: the cleaned first sentence, replaced by a model-written one (`Agent::title`); a
  name the user gave is never replaced.
- `checkpoints.rs`: a snapshot of the whole working tree after each run, as hidden git refs (`refs/viv/checkpoints/...`;
  the user's branch, index and stash are never touched). Gives a run's changed files, its diff, and a rollback that
  restores changed files and removes created ones (a safety checkpoint is taken first, so a rollback can be undone).
  Shell-made changes are included; ignored files are not.
- `prompt.rs`: the coding agent's standing instructions (the old directive without the tools and teams this app
  lacks).
- `resilience/`: retry, backoff and the fallback chain.

Not ported from the old app's sessions: the one-thread-per-day model, per-thread git worktrees, project icons and
banners, the `@file` index.

Not ported from the old loop: key rotation, the long-task ledger (24/7 mode), hooks, the "done means checked"
verification nudge.

- `thread_tools.rs`: `list_threads` and `read_thread`, read-only, limited to the project's own saved conversations.
  Offered to coding agents over MCP.
- `config.rs`: settings in layers (flag, environment, settings.json, the repository's `.viv.json` with repo-safe fields only, default), each with its origin.
- `profiles.rs`: agent profiles from `~/.viv/agents` and the repository's `.viv/agents` (a repository profile cannot set `permission_mode`).

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `core`, `ai`, `providers`, `tools`, `attachments`.
- **State it owns** (R1.6): saved threads (`~/.viv/agent/threads/<id>.json`), settings layers, agent profiles, run checkpoints (hidden git refs in the project) and the task ledger.
- **Check it:** `cargo test -p vivianne-runtime` and `cargo clippy -p vivianne-runtime --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
