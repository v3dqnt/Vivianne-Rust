# runtime/src/query

Reading, filtering, tracing and searching saved conversations. Port of the original `session-query` and
`session-query-sqlite`; the gaps are in `docs/session-query-gaps.md`.

- `mod.rs`: records, errors and the reads (`Query`): list, filter, one event with its neighbours, lineage and event traces.
- `filters.rs`: the filter vocabulary and its checks. Pure.
- `extract.rs`: the text of an event that can be searched. Pure.
- `trace.rs`: lineage and event relationships. Pure over what the reads give it.
- `search.rs`: the full-text index (SQLite FTS5), a database of its own that can always be rebuilt.
