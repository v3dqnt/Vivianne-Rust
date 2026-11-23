# vivianne-multiplayer

Two people on one project from two machines, each with their own agent. The machines connect directly over an encrypted peer-to-peer connection (`iroh`); there is no server. The protocol is the old app's, so a pairing works between this app and the old one.

- `identity.rs`: this machine's key and the code a partner enters.
- `protocol.rs`: length-prefixed JSON frames and the messages. Unknown messages are ignored, not fatal (R5.3), and every decoder is tested with hostile input (R3.2).
- `state.rs`: the pairing state per project.
- `collab.rs`: what each agent is told of the other's work (`before_run`, `after_run`), run summaries, shared usage, and keeping the code in step through a shared git remote or, without one, over the same connection as git bundles.

Prompts and code go only to the partner the person paired with (R4.7).

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `ai`, `core`, `tools`.
- **State it owns** (R1.6): this machine's identity key and the pairings, under `~/.viv`.
- **Check it:** `cargo test -p vivianne-multiplayer` and `cargo clippy -p vivianne-multiplayer --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
