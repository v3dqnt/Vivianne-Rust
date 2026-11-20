# vivianne-tools

The agent's tools. The read-and-search group is Vivianne's **original code, copied**, not rewritten:
`read`, `ls`, `grep`, `glob`, `code_search`, `repo_clone`, `repo_overview`, `diagnostics`, `lsp`.

A tool is `name` + `execute(args, cwd) -> ToolResult { content, is_error, details }`, as in the original.
The model-facing definitions are the original `schemas.rs` entries (`src/schemas.rs`).

## Where the code came from

| Here | Original |
|---|---|
| `src/builtin.rs` | `agent/tools/builtin.rs` (the `Tool` trait, helpers, Grep, CodeSearch, Glob, Diagnostics, gitignore helper, Read, Ls) |
| `src/code/repo_tools.rs`, `semantic_index.rs`, `checkers.rs` | `agent/tools/code/` |
| `src/files/documents.rs`, `doc_design.rs` | `agent/tools/files/` (what `read` parses PDFs, Word, slides and sheets with) |
| `src/lsp/`, `src/lsp_nav.rs`, `src/proc.rs`, `src/code/sandbox.rs`, `src/files/guard.rs` | the 0901 backend: the older `viv-main-gui-exp` copy has no `lsp` tool |

Copied files keep their text and carry a header saying where from; they are marked `rustfmt::skip` in
`lib.rs` so formatting never changes them. Their original `crate::agent::...` paths resolve through the
re-exports in `lib.rs`.

## Stand-ins (written here, small)

The copied code reaches into parts of the old app that are not in this repo. Each gets a stand-in with the
same path and signature:

- `ocr.rs`: image OCR. Reports "unavailable"; `read` already handles that.
- `ai.rs`: stored API keys and the offline embedding models. The search index uses `OPENAI_API_KEY` /
  `NVIDIA_API_KEY` from the environment, or keyword search, as the original does without a stored key.
- `mods.rs`: project trust. Nothing is trusted yet, so servers a project defines for itself do not run;
  the built-in ones do.
- `agent/`: paths under `~/.viv` (never renames anything), an atomic file write, the current turn's thread
  (none yet), the settings file read, and the process-tree helpers.
- `tauri.rs`: the one `spawn` call the language-server reaper makes.

`shell`, `write` and `edit` are simple stand-ins (`shell.rs`, `files_simple.rs`, `simple.rs`) until their
originals are copied as well. Their writes stay inside the project folder (`workspace.rs`).

Not copied: the web search tests, the other tools in `builtin.rs`, desktop control, GitHub, documents
editing. Dependency versions match the original backend.

- `src/open_apps.rs`: the editors on this machine (found on PATH), and opening a file in one at a line, in the
  system's default app, or shown in its folder. Used by the rail's Preview "Open in".
- `src/dev_server.rs`: starts the project's dev server (`dev_server::detect_command` reads package.json and the
  lockfile) and picks the local address out of its output; a server that exits before printing one is reported as
  crashed. Stopped when the app quits.
