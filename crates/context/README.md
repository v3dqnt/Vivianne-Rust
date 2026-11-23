# vivianne-context

What is added to a conversation beside the person's words, as durable user-role messages that are saved, replayed and condensed like any other history. Depends only on `core`; the disk, processes and saved threads come in through small traits, so every rule is tested with made-up inputs. The agent loop (`vivianne-runtime`, `workspace_context.rs`) calls it before each model call.

- `src/instructions/`: AGENTS.md / CLAUDE.md (and `.local.md`) from `~/.viv`, `~/.agents` and the project; baseline, nested files found when a tool reaches a folder, replace and remove notices, per-chain byte budgets.
- `src/time.rs`: the clock reading with offset and IANA zone, which zone applies to unqualified dates, elapsed time.
- `src/tmux.rs`: where the agent sits in tmux, said only when it changes.
- `src/file_ref/`: the `@file` token grammar and a bounded fuzzy search of the workspace.
- `src/session_ref/`: mentions of other conversations and the untrusted snapshot quoted for them.
- `src/dev.rs`: the value-free hint about Vivianne's own checkout.
- `tests/`: behaviour tests for each of the above.

Known deviations from the original: SHA-256 digests, `viv-session:` URIs, `~/.viv` and `~/.agents` homes, synchronous file reads behind the `Fs` trait.
