# Cleanup of `crates/tools/src`

Branch `cleanup/2026-11-23`. Baseline commit `2e9aa11` (snapshot of the working tree before any cleanup).

## Baseline vs now
| | before | after |
|---|---|---|
| files in `crates/tools/src` | 78 (64 `.rs`) | 62 (52 `.rs`) |
| lines of Rust | 22,255 | ~20,870 |
| dependencies in `crates/tools` | 28 | 26 |
| `cargo test -p vivianne-tools` | 206 pass, 7 ignored | 206 pass, 7 ignored (unchanged) |
| `cargo clippy -p vivianne-tools -- -D warnings` | 31 errors | 31 errors (same set; none added) |
| `cargo check --workspace --all-targets` | ok | ok |

Full `cargo build --workspace` crashes rustc at link time on this machine (STATUS_STACK_BUFFER_OVERRUN) both before and after, so `check` was used for the workspace.

## Domains and what was done
- **Old-app stand-ins** (`agent/**`, `ai.rs`, `mods.rs`, `tauri.rs`, `ocr.rs`, 12 files incl. 4 READMEs): merged into `host.rs` (paths, settings, atomic write, trust, current thread) and `proc.rs` (process-tree helpers). `ai.rs` stubs always answered "none"; the search index now reads keys from the environment directly. `ocr.rs` always answered "unavailable"; `read` now says so directly (same message).
- **Stand-in tools** (`shell.rs`, `files_simple.rs`, `workspace.rs`, `simple.rs`): one file, `simple.rs`.
- **lsp**: kept in full (restored on request): the post-edit diagnostics, rename, code actions, lint checks and ledger are the original write/edit pipeline, awaiting the port of `write`/`edit`. They keep their `dead_code, unused` allow.
- **Dead items**: unused constants/functions in `builtin.rs`, `DiagnosticsByFile`, unused client methods and fields. The blanket `dead_code, unused` allow was removed from every file so the compiler now reports dead code.
- **Dependencies**: removed `similar` and `calamine` (no references). `Cargo.lock` was rewritten by cargo, not by hand.
- Stale `src/README.md` and the crate README's stand-in section rewritten.

## Kept although possibly dead (your call)
- `host::turn_thread`/`project_trusted`: seams the new runtime is meant to fill. Return none/false today.
- `Embedder::Local`/`LocalGemma` in `semantic_index.rs`: never produced now, but serialized in existing index caches.
- `dap/**`, `files/documents.rs`, `files/doc_design.rs`, `connections/**`: `pub` items, so rustc cannot prove them unused; not checked against other crates item by item.
- `dap/schemas.rs`, `dap/ui.rs`, `connections/secrets.rs` (<130 lines): natural boundaries, left alone.
- No `cargo machete`/`udeps` available; dependencies checked by grep.

## Behavior that might have changed
- Semantic-index keys: the stored-key lookup always returned none, so only the environment path remains. No change.
