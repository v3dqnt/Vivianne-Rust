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
- `instructions.rs`: AGENTS.md / CLAUDE.md (and `.local.md`, `@imports`) from the user's `~/.viv` and from the project
  up to the repository root, added to the system prompt at the start of every run, with a note when one changed.
- `store.rs`: threads kept on disk (`~/.viv/agent/threads/<id>.json`), listed per project folder.
- `title.rs`: a thread's title: the cleaned first sentence, replaced by a model-written one (`Agent::title`); a
  name the user gave is never replaced.
- `prompt.rs`: the coding agent's standing instructions (the old directive without the tools and teams this app
  lacks).
- `resilience/`: retry, backoff and the fallback chain.

Not ported from the old app's sessions: the one-thread-per-day model, per-thread git worktrees, project icons and
banners, the `@file` index.

Not ported from the old loop: key rotation, the long-task ledger (24/7 mode), hooks, the "done means checked"
verification nudge.
