# remote

Several machines, one window: a port of herdr's multi-machine system. Each machine runs a Vivianne **server** (it owns the threads and agent runs, so work continues when the app closes or the network drops). This app is a **client** that reaches it over the system `ssh` through a small **bridge**, remembers machines in a **catalog**, reconnects by itself and shows what each is doing.

- `acp/`: `viv acp`, the Agent Client Protocol server; see its README.
- `catalog.rs`: the saved machines (`~/.viv/client/machines.json`): id, label, target, session, on/off. No password or key is ever stored; targets with a password or an option-like start are refused.
- `ssh.rs`: the system `ssh` and its options (compression, a managed config that includes the user's first, keepalives, a private control socket off Windows, no prompts and strict host keys for background connections), the remote bridge command and ready marker, error classification (host key / auth / platform / protocol), remote platform detection.
- `protocol.rs`: length-prefixed JSON frames, the messages, the handshake (endpoint generation plus capabilities, not build versions), and the rule that unknown messages are ignored.
- `socket.rs`: the server's local socket (Unix socket 0600, or a named pipe on Windows), one per session.
- `server.rs`: the detached server: threads, agent runs, approvals, per-connection model key held in memory only.
- `bridge.rs`: both ends of the bridge (`viv remote-client-bridge` on the machine; opening a connection from the client).
- `supervisor.rs` and `health.rs`: the retry/backoff and the ping/expiry rules as pure state, with herdr's numbers.
- `client.rs`: one link per machine plus the `Hub` the app reads (`Hub::views`); a down machine keeps its last list, shown dimmed.
- `install.rs`: putting `viv` on a machine, only after a question, checked by SHA-256.
- `detect.rs`: reading a coding agent's state (working / blocked / idle) from its terminal screen with TOML rules.

The `viv-loopback` target runs the bridge on this computer instead of over SSH, for tests and for a second session on one machine.
- `pane.rs`, `panes.rs`: terminal panes the server owns (a pseudo terminal each, the screen as text, the agent's state read from it); `persist.rs` saves them (48 snapshots, 3 backups) and reopens them after a stop.
- `meta.rs`: what is known of each machine's operating system, to pick the remote command (POSIX shell or PowerShell).

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `ai`, `core`, `providers`, `runtime`, `tools`.
- **State it owns** (R1.6): the saved machines (`~/.viv/client/machines.json`) and a server's terminal panes (snapshots, with backups).
- **Check it:** `cargo test -p vivianne-remote` and `cargo clippy -p vivianne-remote --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
