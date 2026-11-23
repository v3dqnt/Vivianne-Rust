# vivianne-cli

`viv [prompt]`: headless chat through `runtime`, no UI. Proves the backend works on its own.
Configure with `VIV_MODEL`, `VIV_PROVIDER`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`.
Keys come from the environment only for now; reading the old app's keychain entry comes later.
- `inspect.rs`: the read-only commands (`status`, `doctor`, `config list`, `sessions`, `session`, `profiles`); `remote_cli.rs`: the machine, server and bridge commands. Every command is listed in `docs/cli-shape.md`.
