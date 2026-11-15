# tools/src/lsp

The language-server client, copied from the 0901 backend. Starts servers on demand, keeps them while
used, and answers `lsp` and `diagnostics`.

- `client.rs`: the protocol. `manager.rs`: which server for which file, idle shutdown, requests.
- `servers.rs`: the built-in server table and the YAML config. `uri.rs`: file URIs and UTF-16 columns.
- `edits.rs`, `check.rs`, `ledger.rs`: applying server edits, project checkers, what the agent was told.
