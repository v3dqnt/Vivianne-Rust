# tools/src

What the agent's tools are made of, by domain.

**The tool box**
- `lib.rs`: `Toolbox`, the list of tools the model sees (and which are offered on a given step).
- `builtin.rs`: the `Tool` trait, `ToolResult` (`ok`, `error`, `failure`), and the read-and-search tools. `schemas.rs`: the model-facing definitions of those. `validate.rs`: arguments are checked against a tool's schema before it runs.
- `simple.rs`: `shell`, `write` and `edit`, kept inside the project folder.
- `results.rs`: a result too big for the model is kept aside and read back in pieces.
- `terminal/`: persistent shells and the `terminal_*` tools; see its README.
- `jobs/`: background jobs and the `job_*` tools; see its README.
- `confinement.rs`: the standing sandbox mode `shell`, `write` and `edit` are held to.
- `workdir.rs`: the directory relative paths and commands start from, shared by the toolbox, `shell` and `run_code`; moved with the `working_directory` tool and kept inside the project.
- `present.rs` (hand files to the user), `run_code.rs` (programs that call the read-only tools) and `ptc/` (the program runtime under it).

**Platform glue**
- `host.rs`: what the tools need from the app (`~/.viv` paths, settings, trust, current thread). `proc.rs`: starting programs without a console window, and ending a process tree.
- `project/`: the project's dev server, and "open in" editors and browsers.

**Folders by domain** (each has a README): `browser`, `computer` (the two MCP servers for the web and the desktop), `code` (repository tools, checks, semantic search), `connections` and `mcp` (apps and servers the agent can use, and their sign-in), `dap` (debugging), `lsp` (language servers: navigation in `lsp/nav.rs`, diagnostics, edits), `files` (documents, the document designer, the write guard, Office to PDF), `git`.

**Tests**: next to what they test. `testing.rs` holds the one helper the tests share (a fresh scratch folder).
