# settings/pages/providers

The Providers page, on real data: nothing here is a sample.

- `mod.rs`: the page, loading what the keys and settings hold, and the ChatGPT plan group.
- `keys.rs`: API keys per provider and the extra keys that take over when one is limited.
- `routing.rs`: cost-aware routing (quick model, difficulty tiers, "try it", savings).
- `custom.rs`: providers the user defines (base URL, format, key, models).
- `agents.rs`: coding agents run through the Agent Client Protocol (which launchers are present).

State lives in `settings/provider_state.rs`; the work itself is `vivianne_ai::providers`,
`vivianne_ai::custom_providers` and `vivianne_ai::inference::routing`.
