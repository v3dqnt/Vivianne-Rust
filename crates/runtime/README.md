# vivianne-runtime

The single agent loop for chats and (later) 24/7 agents. `Agent::run` streams the model, runs the
tools it asks for, feeds results back, and stops when the model answers without tools or hits the
100-step ceiling. Callers watch `AgentEvent`s on a channel; there are no UI or Tauri types here.

`context.rs` splits a request into system prompt, tool definitions, you, assistant and tool results with one
token estimate (four characters), so the parts add up. `Agent::set_mode` changes the instructions and the tools
offered, and a tool the mode withholds is refused even if the model asks for it.

Still to port from the old loop: retry with backoff, context-overflow compaction, repetition guards,
key rotation, long-task ledger (the 24/7 mode), permission prompts.
