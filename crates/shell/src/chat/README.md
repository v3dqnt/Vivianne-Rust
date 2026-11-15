# shell/src/chat

The chat screen. Logic that needs no GPUI is in separate files so it is unit tested.

- `mod.rs`: `ChatView`: layout, key handling, sending, wiring events into the transcript.
- `session.rs`: conversation history plus the agent run on tokio; hands the UI an event channel.
- `items.rs`: transcript items and how agent events change them (tested).
- `fences.rs`: split reply text into prose and code blocks, including a block still streaming (tested).
- `composer.rs`: the text being typed (tested). Append-only for now; cursor and selection come later.
- `transcript.rs`: draws one item: user bubble, assistant prose and code, tool row, notice.
