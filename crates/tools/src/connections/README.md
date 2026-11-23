# tools/src/connections

The apps the agent can connect to in one click (Settings → Connections).

- `catalog.rs`: the list, about 140 apps: hosted MCP servers with self-service sign-in, ones that need no sign-in, ones that take a pasted token, and built-in connectors.
- `oauth.rs`: sign-in with dynamic client registration and PKCE through the browser. `secrets.rs`: tokens go to the OS keychain, with a file in `~/.viv` readable only by the user as the fallback.
- `native.rs`: built-in connectors for services with no hosted server (Slack, Telegram, Discord, WhatsApp).
- `mod.rs`: connect, disconnect, list, and the auth header every request to a connected app carries (refreshed when it is about to expire).

Each connection is an MCP server named by the app's id, so its tools reach the agent as `mcp__<id>__<tool>` under the usual approvals.
