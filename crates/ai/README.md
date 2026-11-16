# vivianne-ai

Vivianne's AI layer, brought over from the old app (`viv-main-gui-exp`, `src/ai/`): all 36 files, about
13,300 lines, with their tests. The code is the old code; it was edited only to cut it loose from Tauri and
the old app's internals, and then cleaned up.

## What is in it

| Area | Files |
|---|---|
| Providers | `provider_registry` (which id is which kind of API), `api/` (a streaming client per kind: Anthropic, OpenAI chat and responses, Google, Azure OpenAI, Bedrock, Mistral, Cloudflare) |
| Keys and sign-in | `auth`, `keychain_store`, `oauth`, `chatgpt` (ChatGPT plan sign-in), `env_api_keys` |
| Models | `models`, `model_discovery`, `pricing`, `types` |
| One-shot calls | `completion` (a single question, plain text back), `embedding` and `local_onnx` (text embeddings, local or remote) |
| Images | `imagegen` (cloud image models and stable-diffusion.cpp) |
| Decision engines | `laya` (local, in a Python environment), `jev` (hosted) |
| Local servers | `local_registry` |
| Shared | `http`, `utils`, `utils_ext` |

## What changed from the old code

The old layer was written for Tauri, which the new app drops.

| Change | Why |
|---|---|
| `tauri::AppHandle` became `Events` (`src/events.rs`) | The in-process bus stream tokens and notices go onto; the agent loop and the UI `subscribe()`. It also keeps shared services such as the credential store. |
| `#[tauri::command]` attributes dropped, `cmd_x` renamed `x` | They were the webview's IPC wrappers; the native app calls the functions directly. In the first 29 files (auth, ChatGPT, discovery) they were thin wrappers and were deleted; in `laya`, `imagegen`, `embedding` and `local_onnx` they held the logic (status, install, download) and stay as public functions. |
| `crate::agent::system::paths` became `crate::paths` | Same `~/.viv` folder, nothing renamed. |
| Reaches into the old agent runtime became `crate::host` | Settings, key lookup, resumable downloads, the memory re-embed event, the model a background call uses. One seam instead of many. |
| `imagegen::run_turn` dropped | It drove the old agent runtime's turn; the picture itself still comes from `generate`. |
| The ignored test in `laya` that needed the old notch router dropped | The router is not part of this app. |

## Clean-code pass

The crate builds with `clippy::all` as warnings and no blanket `allow(unused, dead_code)`. Findings fixed:
module docs written as `//!`, stray `#[allow]`s removed, nested `if let`s collapsed, a Google stream
function that took nine arguments now takes a `GoogleModel`, an `unwrap` after an `is_none` check became a
`match`, a complex type got a name.

## What it needs from the app

- `Events`: stream tokens (`ai-stream-token`), `models_refreshed`, `tool-defs-invalidated`, `memory-reembed`.
- `paths`: the `~/.viv` data folder.
- `host`: `~/.viv/agent/settings.json`, the key for a provider, downloads with progress, the model for background work.

## Not here

`embedding` and `local_onnx` ask the app's memory store to re-embed through the `memory-reembed` event;
there is no memory store in the new app yet, so nothing answers. `host::background_model` returns the model
it is given: cost-aware routing is not part of the new app.
