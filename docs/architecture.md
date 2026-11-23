# Vivianne architecture

## Crates

| Crate | Role |
|---|---|
| `core` | Domain types: threads, messages, plans, memory, attachment references. No I/O. |
| `context` | What is added to a conversation beside the person's words: workspace instruction files, the clock, the tmux pane, mentioned sessions, `@file` completion. |
| `attachments` | Durable images and files attached to messages: content-addressed store, image normalization, request-sized versions. |
| `plugins` | Context, typed services and events, reversible registrations. |
| `providers` | One trait, adapters for OpenAI-compatible, Anthropic, Google, local. |
| `tools` | Shell, browser, desktop control. |
| `runtime` | The single agent loop for chats and 24/7 agents. |
| `app-api` | Local versioned API for micro-app processes. |
| `shell` | GPUI app: window, panels, theme. The only crate that touches GPUI. |

Dependency direction: `shell` -> `runtime` -> `providers`, `tools`, `plugins` -> `core`.

## Phases

0. Crate split and event bus; Tauri events/commands become plain calls; new provider adapters; agent merge. Old app still runs on the same backend.
1. GPUI shell: window, title bar, thread tabs, theme. Spike: (a) streaming markdown transcript at 60fps with code, diff, selection; (b) transparent always-on-top click-through overlays on Windows; (c) terminal via `alacritty_terminal`.
2. Chat transcript and composer. **Go/no-go gate.** If chat feels bad in GPUI, stop.
3. Rail: files, preview, terminal, plan view.
4. Home and Settings.
5. 24/7 agents and debugger.
6. Micro-apps: app API, panels schema, permission UI, mermaid and formulas.
7. Cutover: delete Tauri and the web frontend, MSI, updater.

Each phase is gated by `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`.

## The crate graph (R8.1)

This is the graph `scripts/check-boundaries.sh` enforces. An arrow means "may depend on".

```mermaid
graph TD
  shell --> memory & multiplayer & remote & runtime & providers & tools & telemetry & core & ai & attachments
  cli --> remote & runtime & providers & tools & core & ai & attachments
  remote --> runtime & providers & tools & core & ai
  memory --> tools & core & ai
  multiplayer --> tools & core & ai
  runtime --> providers & tools & core & ai & attachments & context
  attachments --> core
  context --> core
  providers --> core & ai
  telemetry --> ai
  tools
  core
  ai
```

Known gaps between this graph and the documented layering above: `ai` is the real foundation of eight crates though it is not in the original table; `runtime` reads `ai` directly (paths, settings, failure kinds); `core` holds the tool-name lists that decide approvals although `tools` does not depend on `core`; `plugins` and `app-api` have no dependents yet.
