# attachments/tests

Integration tests, one file per concern: `files.rs` (verbatim files, streaming, cancellation, corruption), `images.rs` (limits, normalization, durable save and verified read), `requests.rs` (request versions, cache, single-flight), `prompt.rs` (whole-prompt admission), `hostile.rs` (truncated, oversized and malformed bytes, R3.2). Each test uses a scratch home that is removed afterwards, read-only objects included (R7.3).
