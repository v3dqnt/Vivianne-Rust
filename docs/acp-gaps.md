# ACP: where Vivianne stands

**Client** (Vivianne drives Claude Code, Codex, Gemini CLI and others): `crates/providers/src/acp`. It already had everything the old app's `agent/harness.rs` had, and speaks versions 1 and 2.

**Server** (`viv acp`, other programs drive Vivianne): `crates/remote/src/acp`, a port of `@deepseek-ai/dsh-acp`.

| Original | State |
|---|---|
| `initialize`, `authenticate` | **Ported.** Protocol version 1; no authentication. |
| `session/new`, `list`, `resume`, `close` | **Ported.** Sessions are Vivianne's saved conversations, so the app shows them. Resume must name the same folder. |
| `session/list` pages and cursors | **Not ported:** one page. |
| `session/prompt` with text and resource links; one at a time; `session/cancel` | **Ported.** |
| Image prompts | **Not ported;** refused, and not advertised. |
| `$/cancel_request` | **Not ported.** |
| `session/update`: message, tool call, tool result | **Ported**, sent as the loop produces them. |
| Thought chunks, usage updates, configuration updates | **Not ported.** |
| `session/request_permission` with one-shot choices | **Ported.** Only choosing "allow once" allows; a cancel, an error, anything else or a client that leaves is a no. |
| `session/set_config_option` (model, reasoning effort) | **Not ported.** The model is the one `viv` is set to. |
| MCP servers a session brings | **Not ported:** the list is accepted, not mounted, and named on standard error. The person's own configured servers still apply. |
| A prompt stays open while subagents run | **Not ported** beyond what the loop does itself. Background jobs end with their session and do not start turns. |

Also: version 2 of the protocol is not served. A client that offers 2 is answered with 1.

## Tests

The original's `describe` blocks and ours: permission policy (covered: allow, reject, cancel, client leaving), bridge and prompt lifecycle (covered: one prompt at a time, cancel), codec (stop reasons only through the tests above), rich content (text and links; images refused), connection ownership and multi-session isolation (**not covered**), MCP mapping and model configuration (**not ported**), update projection (covered for the three kinds sent). The original's tests (about 3 500 lines) were not translated.

Checked once by hand: the built `viv acp` against a local stand-in model, writing a file after being asked. No third-party ACP client has been connected to it, and **neither has Vivianne's own** (`providers/src/acp` only starts agents from the registry): that is the first thing to test when ACP delegation is built.
