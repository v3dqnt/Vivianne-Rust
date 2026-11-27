# remote/src/acp

`viv acp`: other programs drive Vivianne's agent over the Agent Client Protocol (version 1) on standard input and output. The other direction, Vivianne driving coding agents, is `crates/providers/src/acp`.

- `mod.rs`: the JSON-RPC loop, requests of the client, shutdown.
- `sessions.rs`: new, list, resume, prompt, cancel, close; asking for permission.
- `updates.rs`: loop events as `session/update`, the permission choices, prompt content.
- `host.rs`: the agent for a folder and where conversations are kept (a trait, so tests use memory).
- `tests.rs`: a client over an in-memory pipe with a scripted model.

Standard output carries only protocol messages.
