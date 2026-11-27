# The original `session-query` package group, and where Vivianne stands

The original group is three packages (about 5 800 lines of source, 7 900 of tests): `session-query` (list, filter, read,
bounded event context and traces over live and saved session logs), `session-query-sqlite` (ranked FTS5 search over them,
as a derived index) and `session-log-export` (Markdown transcript and ZIP). Their base is an event log with typed events,
a "current / shadowed / log-only" surface and parent sessions. Vivianne's conversation is a list of messages, so the
vocabulary is mapped onto that. Code: `crates/runtime/src/query/` and `export_bundle.rs`, `archive.rs`.

| Original | Vivianne | State |
|---|---|---|
| `listSessions`, `filterSessions` (id, cwd, created-at, parent, availability) | `Query::list_threads`, `filter_threads` (id, workspace, updated-at, parent) | **Ported, narrowed.** No `availability` filter: a saved thread is the only kind there is. `updated_at` replaces `created-at` (a thread keeps no creation time). |
| `readSession`, `readSurface`, `listEvents` | `Query::read_thread`, `list_events`, `documents` | **Ported.** Events are the messages, `seq` is the position. The conversation as it stands is `current`; each earlier state kept when it was condensed or rewound (`<id>.g<N>.jsonl.gz`) is `shadowed`. There is no `log-only` surface. |
| `filterEvents` (seq, time, type, surface, literal text) | `Query::filter_events` | **Ported.** Event kinds are `user`, `context`, `assistant`, `tool-result`, `checkpoint`; a tool call is part of its reply. Text is literal, case-blind, white space counts as one. A malformed range or an empty text is refused. |
| Semantic text extraction | `query::extract` | **Ported.** What the app added for the model and condensed history are not searchable; a reply is its words plus its tool names and arguments. |
| `readEvent` with a bounded window | `Query::read_event` | **Ported.** At most 50 either side. |
| `traceSession` (ancestor chain, descendant trees, incomplete when a parent is missing) | `Query::trace_thread`, `query::trace` | **Ported.** A thread now records the thread and turn it was branched from (`parent`, kept through saves by screens that do not know it). Loops are cut and the tree is capped at depth 32 and 500 nodes. Only branches made with `fork` have a parent so far; conversations a subagent or another thread started do not. |
| `traceEvent` (positional replacements, cited sources) | `Query::trace_event` | **Ported, narrowed.** A tool result names the reply that asked for it and a reply names its results; a checkpoint names the earlier state it condensed. There are no positional replacements or citations to follow. |
| Live-preferred observation, prepared-session cache, `readTitleSnapshots`, config knobs | none | **Not needed.** Reads come from the saved file; titles are on the record. |
| `session-query-sqlite`: FTS5 index in a separate database, ranked sessions and events, cursor pagination, reconcile on mismatch | `query::search` (`Index`), `~/.viv/agent/search.db` | **Ported.** `unicode61` tokens, words and phrases only: nothing typed is an operator. bm25 ranking, a best match and snippet per thread, keyset cursors (an invented cursor is refused). Changed threads are re-indexed and gone ones dropped on each reconcile; a damaged or other-version file is deleted and rebuilt. Differences: reconcile reads every thread (no change feed), and the index opens when a search runs (no `openAt`). |
| `session-log-export`: Markdown, ZIP of session, sub-sessions and attachments; Web controls; `/export` | `export_bundle::markdown`, `bundle`, `archive::Zip`; `viv thread export --format` | **Ported as files.** Transcript and ZIP (log, transcript, branches, attachments, manifest), redacted by default, entries named safely, size capped. The Web header menu and slash command are not ported: the app has no export control yet. |
| `session-persistence` interplay (cold read of a crashed turn) | `log` reads | Covered by the log's torn-tail rule (see `session-package-gaps.md`). |
