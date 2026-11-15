# shell/src/chat

The chat screen, built on GPUI Kit components. Logic that needs no GPUI is in separate files so it is
unit tested.

- `mod.rs`: `ChatView`: layout, sending and stopping a run, attaching files, wiring agent events into the
  transcript, keeping the view pinned to the bottom.
- `session.rs`: conversation history plus the agent run on tokio; hands the UI an event channel.
- `composer/`: the composer card (attachments, thinking and model menus, voice, send and stop); see its README.
- `items.rs`: transcript items and how agent events change them (tested).
- `transcript.rs`: draws one item: user bubble, assistant reply as markdown (Kit `TextView`, selectable,
  with code blocks), tool row with scrollable output, notice.

