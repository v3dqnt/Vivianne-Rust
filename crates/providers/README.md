# vivianne-providers

What the agent loop sees of a model provider.

- `lib.rs`: the `Provider` trait and the types it speaks (`ChatRequest`, `ChatEvent`, `ToolSpec`).
- `ai_provider.rs`: `AiProvider`, the old provider layer (`vivianne-ai`) as a `Provider`: Anthropic,
  OpenAI chat and responses, Google, and the Vercel AI Gateway. Streams text, hands back complete tool
  calls, and sends the thinking level to the provider.
- `env.rs`: picks a provider and model from environment variables, until Settings and the keychain do.

## acp/

Other coding agents (Claude Code, Codex, Gemini CLI, the rest of the Agent Client Protocol registry), run as child
processes over stdio. Ported from the newer working layer on the old repo's `updated-threads` branch: protocol v2 with a
v1 fallback, starting an npx agent straight from npx's cache with `node` (the Windows "not recognized" failure), agents
installed under `~/.viv/agents`, signing in (`auth.rs`), resuming sessions, stop that cancels, closes and finally kills,
and stderr cleaned of secrets. See `acp/mod.rs` for the file map. Not ported: offering Vivianne's own tools to the agent
over MCP, delegating a job to an agent from the agent loop, and answering the agent's forms (they are declined).
