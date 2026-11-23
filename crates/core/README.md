# vivianne-core

The domain types every crate shares. No I/O, no async, only `serde`: this crate stays dependency-light on purpose, so anything can depend on it.

- `lib.rs`: `ThreadId`, `Role`, `Message` (text, tool calls, attachment references), `ToolCall`, `Thinking`, and `Mode` (Ask for approval, Auto, Plan: how much the agent may do alone). The lists that decide which tools only look, which ask, and which never have their results saved live here and nowhere else (R1.6); a tool not on the look-only list counts as one that changes things (R4.2).
- `attachment.rs`: durable references to attached images and files (`AttachmentId`, `ImageRef`, `FileRef`, `AttachmentRef`). A reference is an id, a name and facts about the bytes; never a path, a URL or base64. A damaged id still reads (a thread must open) and is refused by the store when the bytes are asked for.
- `debuglog.rs`: the small in-memory log the Diagnostics page shows.

A saved format stays readable: new `Message` fields are `#[serde(default)]` and are left out when empty (R3.5).

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): no other workspace crate.
- **State it owns** (R1.6): none: types only.
- **Check it:** `cargo test -p vivianne-core` and `cargo clippy -p vivianne-core --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
