# hub/src

- `sqlite.rs`: `projects::store::Store` on SQLite; a commit is one transaction.
- `sessions.rs`: bearer tokens, hashed, expiring, revocable.
- `api.rs`: request in, status and JSON out; no sockets. `api_tests.rs` has the over-the-wire scenarios.
- `server.rs`: the HTTP listener with size, time and connection limits.
- `main.rs`: the `viv-hub` command.
