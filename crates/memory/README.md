# vivianne-memory

Project memory, ported from the old app: facts kept per project (`<project>/.viv/memory/memory.db`, SQLite with a
keyword index and optional embeddings) and per user (`~/.viv/user-memory`).

- `src/lib.rs`: the facade: `remember`, `forget`, `recall` (keyword and vector lists merged, gated, optionally
  reranked), per-project on/off state and settings.
- `src/agent.rs`: what the agent loop uses: the `remember`, `recall` and `reflect` tools, `core_block` (standing
  instructions, saved facts and the profile, the only memory in every prompt), and `after_run` (a short note when a run
  changed something, and fact extraction once enough notes build up).
- `src/store.rs`: the SQLite store. `rrf.rs`, `budget.rs`, `temporal.rs`: merging ranked lists, fitting a token budget,
  reading "last spring" as a time window.
- `src/defense.rs`: secrets and card numbers are replaced before anything is written.
- `src/evolve.rs`, `entities.rs`, `observations.rs`, `pages.rs`, `graph.rs`, `reflect.rs`, `rerank.rs`: keeping memory
  true as it grows (reconcile on write, entities, beliefs with their evidence, project pages, links, answering with
  citations, listwise reranking).
- `src/sleep.rs`: tidying while nothing is working (re-embed, episodes, dedupe, forget). `Working::begin()` marks a run.
- `src/hindsight.rs`: an optional connection to a Hindsight server (off by default).

Not ported: the benchmark harness, and the Settings page's buttons (the page exists but is not connected to this crate).

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `ai`, `core`, `tools`.
- **State it owns** (R1.6): per-project memory in `<project>/.viv/memory/memory.db` and the user's memory under `~/.viv/user-memory`.
- **Check it:** `cargo test -p vivianne-memory` and `cargo clippy -p vivianne-memory --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
