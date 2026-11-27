# providers/src/acp

Coding agents (Claude Code, Codex, Gemini CLI and others) run as child processes over the Agent Client Protocol on stdio: finding and installing them, the connection, sessions, turns, permissions, and serving Vivianne's own tools back to them over MCP.

`pipes.rs` runs one turn over given pipes, for an agent inside the process (the server's tests).
