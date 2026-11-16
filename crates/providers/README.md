# vivianne-providers

What the agent loop sees of a model provider.

- `lib.rs`: the `Provider` trait and the types it speaks (`ChatRequest`, `ChatEvent`, `ToolSpec`).
- `ai_provider.rs`: `AiProvider`, the old provider layer (`vivianne-ai`) as a `Provider`: Anthropic,
  OpenAI chat and responses, Google, and the Vercel AI Gateway. Streams text, hands back complete tool
  calls, and sends the thinking level to the provider.
- `env.rs`: picks a provider and model from environment variables, until Settings and the keychain do.
