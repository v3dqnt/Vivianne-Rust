# tools/src/files

Copied from the original `agent/tools/files/` (and `guard.rs` from the 0901 backend).

- `documents.rs`: parsing PDFs, Word, slides, spreadsheets (what `read` uses), and creating them.
- `doc_design.rs`: the document designer behind it.
- `guard.rs`: one writer per file, freshness check, atomic write.
