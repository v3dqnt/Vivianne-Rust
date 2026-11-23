# vivianne-plugins

The plugin context: typed services, typed events and reversible registrations (ADR 0012).

A plugin gets a `Scope` from the `Context` and registers services and event handlers through it. Everything it registers is recorded as an undo step; dropping or disposing the scope runs the steps newest first, so nothing leaks (R3.9).

Single-threaded (`Rc`) on purpose: it lives on the UI thread. Revisit if plugins must run elsewhere. No crate depends on it yet, so it is exercised by its own tests only.

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): no other workspace crate.
- **State it owns** (R1.6): none: a plugin's registrations live in its `Scope` and are undone when it is dropped.
- **Check it:** `cargo test -p vivianne-plugins` and `cargo clippy -p vivianne-plugins --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
