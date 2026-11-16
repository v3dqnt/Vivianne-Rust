# ai/src

- `lib.rs`: declares the copied `ai` module and the two host pieces below it.
- `events.rs`: `Events`, the in-process bus (and kept services) the provider layer sends to.
- `paths.rs`: the `~/.viv` data folder and an atomic JSON write.
- `ai/`: the copied provider layer.
