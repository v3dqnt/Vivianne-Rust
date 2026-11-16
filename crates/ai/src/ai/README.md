# ai/src/ai

The old app's AI layer. See `crates/ai/README.md` for what changed.

- `provider_registry.rs`: which provider id is which kind of API and where it lives.
- `api/`: one streaming implementation per kind of provider.
- `types.rs`, `models.rs`, `pricing.rs`: messages, model info, prices.
- `auth.rs`, `keychain_store.rs`, `oauth.rs`, `chatgpt.rs`, `env_api_keys.rs`: credentials, the OS keychain, OAuth, ChatGPT plan sign-in, keys from the environment.
- `model_discovery.rs`: which models a provider's key actually reaches.
- `http.rs`, `utils.rs`, `utils_ext.rs`: shared helpers.
- `generate_rust.py`: the original generator for the registry and API tables, kept for reference.
- `completion.rs`, `embedding.rs`, `local_onnx.rs`: one-shot completions and text embeddings (local ONNX or remote).
- `imagegen.rs`: image generation, cloud and stable-diffusion.cpp.
- `laya.rs`, `jev.rs`: the decision engines.
- `local_registry.rs`: model-to-address map for local servers.
