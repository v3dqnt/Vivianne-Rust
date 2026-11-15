# tools/src/code

Copied from the original `agent/tools/code/` (and `sandbox.rs` from the 0901 backend).

- `repo_tools.rs`: `repo_clone` and `repo_overview`.
- `semantic_index.rs`: the index behind `code_search` (embeddings with a keyword fallback).
- `checkers.rs`: per-language checkers `diagnostics` falls back on.
- `sandbox.rs`: command checks and the cleaned environment child processes get.
