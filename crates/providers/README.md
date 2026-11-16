# vivianne-providers

The `Provider` trait and two streaming adapters:

- `openai`: OpenAI-compatible chat completions (OpenAI, Google's compatible endpoint, Ollama, llama.cpp).
- `anthropic`: Messages API.

Adapters turn provider JSON into `ChatEvent`s (text, complete tool calls, done). Request building and
stream parsing are pure functions, tested with fixtures; no network in tests.
- `legacy.rs`: the old provider layer (`vivianne-ai`) as a `Provider`. Text only for now: no tools, thinking or retries. `env.rs` picks it for Anthropic and the Vercel gateway.
