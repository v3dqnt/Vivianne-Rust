# vivianne-providers

What the agent loop sees of a model provider.

- `lib.rs`: the `Provider` trait and the types it speaks (`ChatRequest`, `ChatEvent`, `ToolSpec`).
- `ai_provider.rs`: `AiProvider`, the old provider layer (`vivianne-ai`) as a `Provider`: Anthropic,
  OpenAI chat and responses, Google, and the Vercel AI Gateway. Streams text, hands back complete tool
  calls, and sends the thinking level to the provider.
- `env.rs`: picks a provider and model from environment variables, until Settings and the keychain do.

## acp/

Other coding agents (Claude Code, Codex, Gemini CLI, the rest of the Agent Client Protocol registry) run as child
processes over stdio. `registry.rs` lists them and how to start them, `conn.rs` is a running agent and its traffic
trace, `turn.rs` turns what an agent streams into `AcpEvent`s, `mod.rs` has `Acp` (one agent per thread, `run_turn`,
`reset`) plus `install` and `debug`. Nothing in the chat uses it yet: a thread cannot pick an agent as its model.
