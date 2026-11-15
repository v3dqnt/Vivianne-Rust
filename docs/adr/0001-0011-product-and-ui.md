# ADR 0001-0011: product and UI decisions

**0001 Why.** Speed and memory, one language end to end, native look and feel.

**0002 Target.** Fully native GPU UI on GPUI (Zed's framework). No webview at all.

**0003 Look.** Fluent on Windows, liquid glass on macOS. GPUI draws everything, so Fluent is a theme
built on top (`gpui-component` is a head start; Mica/acrylic backdrop is possible). Liquid glass needs
custom drawing, so macOS comes after Windows. First release is Windows only.

**0004 Rollout.** Phases, not a big bang. The old Tauri app keeps working until parity.

**0005 Tauri dropped.** The backend stops depending on `tauri::AppHandle`, events and commands. The
native app calls the backend in-process.

**0006 Backend refactor.** Done together with the frontend. Folds in the approved agent merge: agents
are a second copy of the chat runtime; one loop in `runtime` serves both.

**0007 Providers.** Replace the generated provider layer (`ai/api/*`, `provider_registry.rs`) with a
small set of Rust adapters behind one trait: OpenAI-compatible, Anthropic, Google, local. Later:
Vivianne's own hosted gateway (keys, routing, billing).

**0008 Old UI.** The web UI and HeroUI library are frozen to bug fixes. Library tokens (colours,
6/10px radii, type scale) become the GPUI theme.

**0009 Verification.** A GPUI `gallery` binary (one screen per component) plus a screenshot step.

**0010 Rich content.** Robot heads: custom 3D models via `wgpu`. Mermaid and formulas: later.
Terminal: `alacritty_terminal`. Code colour: `syntect` or tree-sitter. Markdown: `pulldown-cmark`
with custom layout.

**0011 Mods.** Old HTML/iframe mods are dropped, replaced by micro-apps (see 0012).
