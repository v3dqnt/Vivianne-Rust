# Plan: usage tracking and telemetry

Port both layers from the old app (repo `C:\Users\v3dqn\Work\Project Versa\viv-main-gui-exp`, branch
`updated-threads`, read with `git show updated-threads:<path>`) and fix what was wrong with them.

Old sources:
- usage: `packages/gui/src-tauri/src/agent/models/usage_store.rs` (already copied to
  `crates/ai/src/inference/usage_store.rs`), commands `cmd_get_usage_history`, `cmd_get_usage_by_model`,
  `cmd_session_cost` in `packages/gui/src-tauri/src/agent/runtime/commands.rs` (~line 1478),
  `record_usage` in `.../agent/orchestration.rs` (~line 566), UI `packages/gui-next/src/modules/settings/components/UsageSection.tsx`
  and `UsageByModel.tsx`.
- telemetry: `packages/gui/src-tauri/src/agent/system/telemetry.rs`, commands `cmd_telemetry_track`,
  `cmd_get_telemetry_summary`, `cmd_clear_telemetry`, `cmd_submit_bug_report` in `commands.rs` (~line 3080), the panic
  hook and `app.start` event in `packages/gui/src-tauri/src/lib.rs` (~line 591), UI
  `packages/gui-next/src/modules/settings/components/DiagnosticsSection.tsx`.

## Where things stand in the new app

- `usage_store` is copied but half wired: `vivianne-providers` (`ai_provider.rs::record_usage`) and the ACP layer
  (`providers/src/acp/turn.rs::record_usage`) call `record_usage` with no source tag, so nothing knows which thread
  spent what.
- Settings, Account shows a usage heatmap and a by-model table from `Values.usage_days` / `model_usage`, which are
  now always empty (the sample data was removed). Settings, Diagnostics is in memory and empty.
- There is no telemetry module at all: no events, no crash records, no bug reports.

## Bugs and weak spots in the old code, to fix while porting

Usage:
1. The buffer is only flushed when a later record arrives after the flush interval, and nothing calls `flush_all`
   on quit: the last minutes of usage are lost every time the app closes. Flush on a timer and on quit.
2. Files are written with `tokio::fs::write`, which truncates first: a crash mid-write loses the whole history.
   Write through a temp file and rename (`vivianne_ai::paths::write_json_atomic`).
3. `usage_by_model.json` is written by the by-source writer, so it inherits the 90-day by-source retention: the
   "All time" by-model view silently only covers 90 days. Give by-model its own retention (keep it all, or a
   year; say which in the module doc).
4. Usage is recorded where the model call ends (the provider), which has no idea of the thread. Move recording to
   where the thread is known: the turn's usage travels up as an event and the session records it with a
   `main:<thread>` tag. Recording in the provider stops (no double counting).

Telemetry:
5. `append_event` reads and rewrites the whole file after every append to trim it: O(n) per event. Trim only when the
   file has grown well past the cap, decided from its size, not by reading it every time.
6. Appends from several threads are not serialised: lines can interleave. One process-wide lock around appends.
7. `props` is free-form: callers could put prompt text, file paths or keys in it. Keep props to a short allowlist of
   value types (numbers, booleans, short identifiers), cap their size, and never record message content.
8. Upload builds a new HTTP client per call and accepts any URL. Use the shared client
   (`vivianne_ai::ai::http::client()`), require `https://`, and keep upload strictly opt-in
   (`telemetryEnabled` true **and** `telemetryEndpoint` set; both default off).
9. Crash files are written with a plain write; use the atomic write.

## What to build

### A. Usage tracking (`crates/ai/src/inference/usage_store.rs`, `crates/providers`, `crates/runtime`, `crates/shell`)

- Fix 1 to 3 above. Add `pub async fn flush_all()` callers: a periodic flush task started by the shell backend, and
  a flush when the app quits (GPUI `on_app_quit` or the window closing, whichever exists; look it up).
- Usage travels with the turn (fix 4): add the turn's usage to what the provider reports (e.g. a `ChatEvent::Usage`
  or usage on `Done` in `crates/providers/src/lib.rs`, filled from the final `AssistantMessage.usage` in
  `ai_provider.rs`), sum it per turn in `crates/runtime/src/agent.rs`, and emit `AgentEvent::Usage { provider, model,
  input, output, cache_read, cache_write, cost: Option<f64> }`. The shell's `chat/session.rs` records it with
  `record_usage_tagged(.., "main:<thread>")` plus `record_usage`. The ACP turn does the same through its own path
  (tag it `main:<thread>`, provider `harness`). Remove the provider-level recording.
- A per-thread total: `usage_store::thread_total(thread) -> DayUsage` (sum of `main:<thread>` and `task:<thread>`
  tags across days, as old `cmd_session_cost` did). Show it in the chat as a small "this thread: N tokens, $X" line
  under the composer or in the status row (the chat's composer view has a status area: look in
  `crates/shell/src/chat/composer/view.rs`). Unknown cost says so, never $0.
- Settings, Account: read real data. Heatmap from `read_history()`, by-model table from
  `read_usage_by_model(since)` for the range picker (7 / 30 / all). Load when the page is first drawn, the way the
  Providers page does (`crates/shell/src/settings/pages/providers/mod.rs`: `start_loading`, `backend::run`). Keep
  the page's existing look; replace only where its numbers come from. Map `DayUsage` into the page's `UsageDay` /
  `ModelUsage` (they live in `crates/shell/src/settings/values.rs`).

### B. Telemetry (new crate `crates/telemetry`, `vivianne-telemetry`)

- Port `telemetry.rs` into the new crate with fixes 5 to 9: `track(kind, props)`, `read_events()`,
  `install_panic_hook(version)`, `read_crashes()`, `clear()`, `summary()` (counts by kind, first and last event
  time, crashes), `submit_report(description, include_diagnostics) -> Report { path, uploaded }` (always written to
  `~/.viv/agent/reports/` first), and opt-in `upload`. Same files as the old app (`~/.viv/agent/telemetry.jsonl`,
  `~/.viv/agent/crashes/`), so existing data is read.
- Hook it up: install the panic hook first thing in `crates/shell/src/main.rs`; `track` a small fixed set of events
  with no content: `app.start` (version, OS), `thread.new`, `turn.complete` (provider, model, ok or error kind,
  duration ms, tool count), `tool.call` (tool name, ok), `model.switch`, `agent.sign_in` (agent id, outcome),
  `settings.open` (page id). Add them where those things happen; keep each call one line.
- Settings, Diagnostics (`crates/shell/src/settings/pages/diagnostics.rs`): real data. Sharing switch and endpoint
  read and saved through `vivianne_ai::host::update_settings` (`telemetryEnabled`, `telemetryEndpoint`); recorded
  events summary (count, since, counts by kind); crashes with expandable traces; Clear button (asks first, it
  deletes); report form whose Send writes the report and says where it went and whether it was uploaded. The page
  currently says reporting isn't connected: that goes.

## Rules for the work

- Every folder has a README.md; update the ones you touch and write one for the new crate.
- Workspace lints are strict (pedantic warn, `unwrap_used` deny). `cargo clippy --workspace --tests` must have no
  errors and no new warnings in the files you write.
- Tests for the logic that is not UI: flush and retention, the tag sum, the props allowlist, the trim, the opt-in
  default (no upload without both settings), report written even when upload fails.
- On this Windows machine the bash tool unescapes backslashes in heredocs: write files with the Write tool, not
  heredocs.
- Don't commit, don't push, don't start the app.
