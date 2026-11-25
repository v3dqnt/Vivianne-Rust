# vivianne-cli

The `viv` command: the app's backend without the window. It proves the backend works on its own and gives scripts, SSH sessions and the remote server one code path with the app (R1.4).

- `main.rs`: wires only (R1.2). `viv [prompt]` is a headless chat through `runtime`; the model comes from `VIV_MODEL`, `VIV_PROVIDER`, and the provider's key variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`).
- `inspect.rs`: the read-only commands (`status`, `doctor`, `config list [--show-origin]`, `sessions`, `session`, `profiles`, `thread list|events|fork|mention`), with text and `--json` from one snapshot (R5.2).
- `remote_cli.rs`: the machine, server and bridge commands (`machine`, `server`, `remote-client-bridge`).

Every command, flag, setting, environment variable and exit code is listed in `docs/cli-shape.md` (R5.1). Results go to stdout, diagnostics to stderr, and a usage error exits 2.

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `core`, `providers`, `runtime`, `tools`, `remote`, `ai`, `attachments`.
- **State it owns** (R1.6): none of its own: it wires the other crates behind the `viv` command.
- **Check it:** `cargo test -p vivianne-cli` and `cargo clippy -p vivianne-cli --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
