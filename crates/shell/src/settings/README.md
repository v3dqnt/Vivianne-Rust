# shell/src/settings

The Settings dialog (Kit `Settings`): a page list on the left, the page on the right, in the old app's order
(Viv Instant left out on purpose).

- `mod.rs`: opens the dialog.
- `pages/`: one module per page; `browser.rs` supplies the agent-browser groups of the Web & browser page.
- `values.rs`: the values the controls edit. Only the Providers page is saved to where the old app kept its settings; the rest is in memory. Saving
  into `~/.viv` (the old app's format) comes with the settings store.
- `defaults.rs`: the app's own defaults and catalogs (shortcuts, limits, optional components, starter models). No sample data.
- `widgets.rs`: shared building blocks (text styles, switch/dropdown/input fields bound to `Values`, buttons).
- `provider_state.rs`: what the Providers page shows and edits, read from where the keys and settings live.
