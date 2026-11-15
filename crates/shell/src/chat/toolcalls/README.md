# shell/src/chat/toolcalls

One view per tool call in the transcript: a header (icon, what ran, on what, a result chip) and a body made
for that tool's output. Click the header to fold it.

- `mod.rs`: the header and frame, chosen by tool name, built from the tool's args and `details`.
- `bodies.rs`: the bodies: numbered code (`read`), tree (`ls`), matches by file (`grep`), file list (`glob`),
  ranked regions (`code_search`), facts (`repo_clone`, `repo_overview`), problems (`diagnostics`), diff
  (`edit`). Anything unrecognised falls back to plain output.
- `parse.rs`: reads each original tool's text format into those shapes. No GPUI, so it is unit tested.
