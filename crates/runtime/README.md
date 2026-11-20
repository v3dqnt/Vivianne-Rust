# vivianne-runtime

The single agent loop for chats and (later) 24/7 agents. `Agent::run` streams the model, runs the
tools it asks for, feeds results back, and stops when the model answers without tools or hits the
100-step ceiling. Callers watch `AgentEvent`s on a channel; there are no UI or Tauri types here.

`context.rs` splits a request into system prompt, tool definitions, you, assistant and tool results with one
token estimate (four characters), so the parts add up. `Agent::set_mode`: Plan offers only tools that look (and refuses any other even if asked); Ask emits
`AgentEvent::Approval` and waits for `Agent::answer` before a tool that changes things; Auto (the default here,
since a library caller has nobody to ask) just runs.

Still to port from the old loop: retry with backoff, context-overflow compaction, repetition guards,
key rotation, long-task ledger (the 24/7 mode), permission prompts.
