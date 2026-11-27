# The original `guard` packages, and where Vivianne stands

| Original | Vivianne | State |
|---|---|---|
| `repeat-tool-reminder`: a nudge at 3, 5 and 8 consecutive identical calls, reset by the person | `crates/runtime/src/repeats.rs` | **Ported, narrowed.** The note is added to the tool result rather than sent as its own context message. No include or exclude patterns; thresholds and the 500-character preview are fixed. |
| `timeout-policy`: a tool with a declared budget is cut off with `tool call timed out after Nms` | the agent loop (`Tool::timeout`, `with_tool_timeout`) | **Already there.** The wording differs, there is no `TOOL_TIMEOUT` code, and the call is dropped rather than asked to stop. |

## Tests

The originals have about 30. Here: three for the reminder (the thresholds, what resets the count and key order, the preview) and the loop's existing timeout test. Not covered: that the note reaches a real conversation, a denied call counting, per-agent separation (each run has its own count).
