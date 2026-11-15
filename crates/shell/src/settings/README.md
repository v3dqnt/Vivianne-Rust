# shell/src/settings

The Settings dialog (Kit `Settings`): a page list on the left, the page on the right, in the old app's order
(Viv Instant left out on purpose).

- `mod.rs`: opens the dialog.
- `pages.rs`: one function per page. Reliability, Web & browser, Memory, Diagnostics, Appearance and Layout have
  controls; the rest say "not built yet".
- `values.rs`: the values the controls edit. **In memory only for now**: nothing is saved or applied. Saving
  into `~/.viv` (the old app's format) comes with the settings store.
