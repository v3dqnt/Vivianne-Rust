# shell/src/settings/pages

One module per Settings page, each exposing `page(values) -> SettingPage`. `mod.rs` lists them in the old app's
order: account, providers, local, reliability, mods, connections, web, memory, vault, skills, diagnostics,
appearance, layout, components, updates.

Every page edits the in-memory `Values`; nothing is saved or applied yet.
