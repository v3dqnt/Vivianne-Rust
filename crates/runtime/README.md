# vivianne-runtime

The single agent loop for chats and (later) 24/7 agents. `Agent::run` streams the model, runs the
tools it asks for, feeds results back, and stops when the model answers without tools or hits the
100-step ceiling. Callers watch `AgentEvent`s on a channel; there are no UI or Tauri types here.

Still to port from the old loop: retry with backoff, context-overflow compaction, repetition guards,
key rotation, long-task ledger (the 24/7 mode), permission prompts.
