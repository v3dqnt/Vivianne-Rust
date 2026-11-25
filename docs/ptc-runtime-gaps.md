# The original `ptc-runtime` group, and where Vivianne stands

The original `packages/ptc-runtime` is the execution seam under programmatic tool calling: `ptc-runtime` (the service
definition: what a program is handed and what it gives back), `ptc-runtime-node` (a Node provider with process isolation,
file sandbox policy, retained REPL state and a wire protocol) and an experimental Python backend. Vivianne had a
`run_code` tool with its own one-shot engine and none of the contract around it. `tools::ptc` is the contract and a QuickJS
provider; `tools::run_code` now runs on it.

| Original | Vivianne | State |
|---|---|---|
| Request, spec and result vocabulary; a failed program is a field on the result | `ptc::types` (`RunRequest`, `RunSpec`, `RunResult`, `Failure`) | **Ported.** |
| Eight failure kinds: exception, timeout, abort, worker-exit, invalid-output, output-limit, protocol, sandbox-unavailable | `ptc::FailureKind` | **Ported.** `sandbox-unavailable` exists but nothing raises it: a program has no file access to confine. |
| `resolve` then `run`: defaults, deadline cap, `timeoutMs: null`, absolute `cwd` | `Runtime::resolve` / `run` | **Ported.** |
| Binding namespaces: portable identifiers, reserved words, reserved globals, unique names | `ptc::reserved` | **Ported** with the original's sets (ECMAScript and Python words, `console`, `__dsh_main__` and the rest). |
| Typed rejection class per namespace (`errorClass`, member property, reserved error members, dunder names) | `BindingErrorClass`, prelude `__install` | **Ported.** A rejected call is an instance of the class with the member's name on it. |
| Null-prototype namespaces; `__proto__` and `constructor` are ordinary member names | prelude | **Ported.** |
| Async bindings the program awaits; several started together run together; `maxPendingCalls` | `engine::Session::settle` | **Ported.** Calls the program starts in one turn are answered together; over the limit is a `protocol` failure. A call nothing awaits is not answered after the program ends. |
| Lossless-JSON boundary (`json-wire`): refuses undefined, functions, symbols, bigint, NaN/Infinity, -0, class instances, sparse arrays, accessors, hidden and symbol keys, cycles, depth | prelude `__snapshot` | **Ported.** Arguments to calls are checked the same way. Depth limit 1000. Not ported: defences against a program that replaces `Object.prototype` internals, because the checks capture the intrinsics before the program runs. |
| Output ledger: logs, value and diagnostic share one byte cap, exact JSON accounting, a fitting log prefix kept | `ptc::output` | **Ported.** |
| `maxMessageBytes` for control messages and call arguments | `Limits::max_message_bytes` | **Ported** for call arguments. |
| Deadline covers setup, execution and nested waits; "before code started" vs "during execution" | `Session::verdict` | **Ported.** |
| Abort signal stops the program mid-loop | `RunRequest::cancel` | **Ported.** |
| Memory and stack limits | QuickJS `set_memory_limit` / `set_max_stack_size` | Same job (the original limits the V8 old generation). |
| Per-call mode: async function bodies, fresh process each run | `ExecutionMode::PerCall` | **Ported** (a fresh environment on its own thread, not a process). |
| Persistent REPL mode: one environment per state owner, `release(owner)`, concurrent runs of one owner refused | `ExecutionMode::Persistent`, `OwnerId`, `Runtime::release` | **Ported.** A run stopped from outside (timeout, abort, output limit, protocol) discards the environment and says its variables were cleared; an exception keeps earlier changes. Different: no top-level `await` (a promise as the last value is awaited). |
| Script mode (`vm-script`) with optional `scriptPersistence` | none | Open: QuickJS has no module-less top-level-await transform; REPL mode covers retained state. |
| File sandbox policy, OS confinement, `sandboxMode`, `denied` | none | Not needed: the engine has no file, network or process access. |
| Process isolation, wire protocol, managed process range, `stopGraceMs`, `launch` | one thread per environment | Not needed in-process. |
| TypeScript source (erasable types) | JavaScript only | Different. |
| Python backend (experimental, private) | none | Not ported. |
