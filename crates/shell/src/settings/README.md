# shell/src/settings

The Settings dialog (Kit `Settings`): a page list on the left, the page on the right, in the old app's order
(Viv Instant left out on purpose).

- `mod.rs`: opens the dialog.
- `pages/`: one module per page (Account, Providers, Local models, Reliability, Mods, Connections, Web & browser,
  Memory, Vault, Skills, Diagnostics, Appearance, Layout, Components, Updates). `pages/mod.rs` lists them in order.
- `values.rs`: the values the controls edit. **In memory only for now**: nothing is saved or applied. Saving
  into `~/.viv` (the old app's format) comes with the settings store.
- `demo.rs`: the sample data that fills `Values` until real data is wired in.
- `widgets.rs`: shared building blocks (text styles, switch/dropdown/input fields bound to `Values`, buttons).
- `provider_state.rs`: what the Providers page shows and edits, read from where the keys and settings live.
