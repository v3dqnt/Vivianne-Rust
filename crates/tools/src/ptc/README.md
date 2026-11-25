Program execution (port of the original `ptc-runtime` and `ptc-runtime-node`): run one model-written program against host async functions and report what it printed and returned.

- `types.rs`: what a caller asks for and gets back. A failed program is a field on the result (`FailureKind`: exception, timeout, abort, worker-exit, invalid-output, output-limit, protocol, sandbox-unavailable), never an error of `run`.
- `reserved.rs`: the names every backend refuses (reserved words, backend globals, error members) and the check applied to a binding list.
- `output.rs`: exact byte accounting of logs, value and diagnostic against one cap.
- `engine.rs` and `prelude.js`: one QuickJS environment and the loop that drives a program in it: async bindings answered together, deadline, cancellation, the lossless-JSON check, typed binding errors.
- `mod.rs`: `Runtime` (resolve, run, release, dispose) and its two modes: `PerCall` (async function bodies, a fresh environment each run) and `Persistent` (a REPL environment kept per owner).
- `tests.rs`: the runtime against real programs and made-up host functions.

Deviations: the engine is QuickJS (JavaScript, not TypeScript; no Python backend), there is no operating-system file sandbox because a program has no file access of its own, and the original's `script` format and per-process heap flags are not ported. A REPL program waits for its last value if that is a promise; it has no top-level `await`.
