# vivianne-ai

Vivianne's provider layer, **copied from the old app** (`viv-main-gui-exp`, `src/ai/`), not rewritten:
the provider registry, one streaming API per kind of provider (Anthropic, OpenAI chat and responses,
Google, Azure OpenAI, Bedrock, Mistral, Cloudflare), credentials and the OS keychain, ChatGPT plan
sign-in, model discovery, pricing, env-var keys. About 12,000 lines; its 71 original tests pass.

This replaces ADR 0007 (small hand-written adapters). The earlier `vivianne-providers` crate is still
what `runtime` and `cli` use; wiring them to this layer is a separate step.

## What was left out

`imagegen`, `laya`, `jev`, `local_onnx`, `local_registry`, `embedding` (image generation, Laya, Jev and the
offline embedding models: not provider code) and `completion` (background LLM calls routed by agent
logic). The list is in `mod.rs`, which lost those seven lines.

## What was changed (all of it)

Everything else is byte for byte the original. The old layer was written for Tauri, which the new app
drops, so only the places where it touched Tauri or the old app were edited, by rules in a script and
counted:

| Count | Change |
|---|---|
| 24 | `tauri::AppHandle` / `AppHandle` becomes `Events` (our own handle, `src/events.rs`) |
| 7 | `use tauri::...` lines dropped or replaced by `use crate::events::Events;` |
| 17 | `#[tauri::command]` functions deleted: they were the old webview's IPC wrappers around code that a native app calls directly (`auth_*`, `cmd_*`) |
| 3 | `crate::agent::system::paths::` becomes `crate::paths::` (`src/paths.rs`, same `~/.viv`, never renames) |
| 1 | ChatGPT sign-in opens the browser with `open::that` instead of Tauri's shell plugin |
| 1 | ChatGPT sign-in emits `tool-defs-invalidated` instead of calling the old agent's cache directly |
| 1 | `State::inner().clone()` becomes `(*state).clone()` (a kept service is an `Arc`, not Tauri's `State`) |

The crate builds with edition 2021, as the old code did, and with its lints relaxed so the copy keeps
its style.

## What it needs from the host

- `Events`: stream tokens (`ai-stream-token`), `models_refreshed` and `tool-defs-invalidated` go onto its bus;
  the agent loop and UI `subscribe()`. It also keeps shared services such as the credential store.
- `paths`: the `~/.viv` data folder.

The 0901 backend's copy of this layer differs in six files (`api/anthropic.rs`, `model_discovery.rs`,
`types.rs`, `mod.rs` and two of the left-out modules). This copy is from `viv-main-gui-exp`.
