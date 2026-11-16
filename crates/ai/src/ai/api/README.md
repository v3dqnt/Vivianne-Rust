# ai/src/ai/api

One streaming implementation per kind of provider, copied from the old app: `anthropic`, `openai_completions`,
`openai_responses`, `google` (with `google_shared`), `azure_openai`, `bedrock`, `mistral`, `cloudflare`, plus
shared pieces (`openai_shared`, `transform_messages`, `simple_options`, headers). Each streams tokens as
`ai-stream-token` events on `Events`.
