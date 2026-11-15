# Vivianne architecture

## Crates

| Crate | Role |
|---|---|
| `core` | Domain types: threads, messages, plans, memory. No I/O. |
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
