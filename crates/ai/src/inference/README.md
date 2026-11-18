# ai/src/inference

Which model does the work, and what it costs. Brought over from the old app's `agent/models/` and `models/`,
with the Tauri commands turned into plain functions that take the app's `Events` bus.

- `difficulty.rs`: how hard a task reads (rules, then Laya or Jev when installed).
- `routing.rs`: cost-aware choice of a quick model, the "Auto" router, and the savings meter.
- `usage_store.rs`: tokens and cost per day, per model and per source, kept under `~/.viv/agent`. Every finished
  turn in `vivianne-providers` records into it.
- `local_models.rs`: what this computer can run, GGUF reading, downloads into `~/.viv/models`, installing llama.cpp.
- `llama_manager.rs`: starts, watches and stops local `llama-server` / `mistralrs` processes.

Downloads share `host::download`. Where the old code asked the agent runtime for something, it now takes it as an
argument (`routing::is_read_only` takes the tool check; `routing::cmd_routing_savings` takes the chat model).
