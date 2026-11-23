# browser

Browser control: the agent opens web pages, reads them as an accessibility tree and acts on elements by reference, through `agent-browser` (vercel-labs, Apache-2.0), run as an MCP server named `browser`. Its tools reach the agent as `mcp__browser__agent_browser_<tool>`.

- `mod.rs`: the settings (on/off, auto, show window, allowed sites), the 25 tools the model is offered (`OFFERED`), the server's environment (own profile, content boundaries, output cap, the system Chrome or Edge), `apply()`, and the agent's instructions (`GUIDE`).
- `guard.rs`: every call goes through it: arguments the model may not set are removed, one session is forced, only http(s) addresses open, secrets are not typed, and results are marked as untrusted web content and capped.
- `install.rs`: finds the program, downloads the release file for this machine into `~/.viv/bin` after a SHA-256 check, and reports its version.

Which tools ask for approval and which results are never saved is decided in `vivianne_core` (`is_look_only`, `returns_desktop_content`, `browser_navigates`); old snapshots are masked in `vivianne_runtime::history`. A web page is text from strangers, so opening a page asks even in Auto mode unless the user limited the sites.
