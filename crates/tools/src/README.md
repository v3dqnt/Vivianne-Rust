# tools/src

- `lib.rs`: `Toolbox`, the tool list the model sees.
- `builtin.rs`: the `Tool` trait and the read-and-search tools. `schemas.rs`: the model-facing definitions.
- `shell.rs`, `files_simple.rs`, `simple.rs`, `workspace.rs`: shell, write and edit, kept inside the project folder.
- `host.rs`: what the copied tools need from the old app (`~/.viv` paths, settings, trust, current thread).
- `proc.rs`: starting programs without a console window, and ending a process tree.
- `results.rs`, `open_apps.rs`, `dev_server.rs`: oversized results, "open in" editors, the project's dev server.
- Folders: `browser`, `code`, `computer`, `connections`, `dap`, `files`, `git`, `lsp`, `mcp`; each has a README.
