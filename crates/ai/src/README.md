# ai/src

- `lib.rs`: declares the `ai` module and the three host pieces beside it.
- `events.rs`: `Events`, the in-process bus (and kept services) the AI layer sends to.
- `paths.rs`: the `~/.viv` data folder and an atomic JSON write.
- `host.rs`: what the layer needs from the app: settings, the key for a provider, downloads, the background model.
- `ai/`: the old app's AI layer.
