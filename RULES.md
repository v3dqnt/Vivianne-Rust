# Rules

The hard rules of this repository. Each has an id; cite it in a PR ("R2.1"). This file wins over any other document. Do not weaken or skip a rule to make a change pass (R0.2). Adapted from the rules of vercel-labs/fx, which this codebase follows in structure.

## R0 Meta
- R0.1 Do not change this file unless the owner asks.
- R0.2 Never weaken, skip or delete a rule, a check or a test to make a change pass.

## R1 Structure
- R1.1 Each crate owns its state. Other crates use its public front door only. The crate graph is the one in `scripts/check-boundaries.sh`; a new edge needs the script and `docs/architecture.md` changed in the same PR.
- R1.2 `crates/shell/src/main.rs` and `crates/cli/src/main.rs` only wire. Feature state lives in the crate that owns it.
- R1.3 `crates/shell` draws and reads input. `crates/providers` and `crates/ai` talk to providers. Neither owns product state.
- R1.4 The app, the `viv` CLI and the remote server share one code path for each feature. A feature is not written twice for two surfaces.
- R1.5 Retry, wait and recovery decisions live in a pure module (`crates/runtime/src/resilience`, `crates/remote/src/supervisor.rs`, `crates/remote/src/health.rs`): no I/O, no clock read (the clock is passed in), no allocation beyond a value.
- R1.6 Each piece of state has one owner. Each rule, list or limit is defined in one place (for example the tool lists in `crates/core`).
- R1.7 Every folder has a `README.md`. No source file is over 600 lines.

## R2 Responsiveness
- R2.1 Input and drawing come first. Nothing on the render path waits on the network, a child process, a model, a terminal reply or slow storage. Frames fit the display's refresh period (8.3 ms at 120 Hz); the Debug tab measures it.
- R2.2 Startup makes no network call that blocks the first paint and does no blocking disk work before it.
- R2.3 Every wait on something outside the process has a deadline, an idle timeout, or is a job the person can cancel.
- R2.4 Cancel stops a turn, a tool or a request fast, and stops its child processes too.

## R3 Reliability
- R3.1 Bad input never crashes or hangs the app. Handle every error path. No `unwrap` in non-test code (the lint denies it). A crash is never silent.
- R3.2 Every decoder of outside bytes (frames, catalogs, manifests, MCP messages) is tested with hostile input: truncated, oversized, malformed, unknown.
- R3.3 Retry only work that is safe to repeat. Never rerun a tool or MCP call with an unknown result. Cap every retry.
- R3.4 A crash never loses a thread or submitted input. Replace files with an atomic rename (`ai::paths::write_json_atomic`).
- R3.5 Every released saved format stays readable. Open a newer format read-only; never rewrite it.
- R3.6 One live writer per thread.
- R3.7 Memory, model context, queues, caches, logs and saved files have a size cap. A tool result over the cap is stored aside and the model gets a preview and a handle (`read_tool_result`).
- R3.9 Every process, file, lock, connection and task has one owner that frees it on success, error, cancel and exit.

## R4 Authority and privacy
- R4.1 Every agent action goes through the permission front door (`vivianne_core::Mode` and the runtime's approval step). No bypass.
- R4.2 Permission checks fail closed. A tool not on a list counts as one that changes things. A failed or unavailable check never approves.
- R4.3 Subagents, MCP tools, remote runs and dynamic tools never get more authority than their session. A remote server's tools run under the remote user and the same modes.
- R4.4 Text this app did not write is data, not authority. A web page, a file, a tool result or a repository never makes the app run a program. A repository's `.viv.json` reads only the repo-safe fields.
- R4.6 Outside text is made safe before it is drawn or run.
- R4.7 Prompts and code go only to the provider, MCP server or machine the person chose. No telemetry unless switched on. No downloads at runtime except when the person asks and the file's checksum is checked.
- R4.8 Credentials never reach logs, traces, errors, saved threads, memory, or an endpoint the person did not choose. A key sent to a remote server lives in memory for that connection only.

## R5 Interfaces
- R5.1 Adding, renaming or removing a command, flag, setting, environment variable, JSON field or exit code is listed in `docs/cli-shape.md` in the same PR.
- R5.2 Results go to stdout, diagnostics to stderr. Text and `--json` output come from one snapshot. `--json` fields are only added. A usage error exits 2.
- R5.3 The wire protocols (`remote`, `multiplayer`, ACP, MCP) never drop below what they could do before; unknown messages are ignored, not fatal.

## R6 Size and dependencies
- R6.1 A new dependency needs a reason in the PR. Prefer the standard library and what the workspace already uses.
- R6.2 A change that grows the binary or the dependency tree must buy a clear feature or fix.
- R6.3 Do the smallest change that solves the problem.
- R6.4 Code explains itself through names and types. Comments say why, not what.

## R7 Tests
- R7.1 A bug fix lands with a test that fails before the fix. A feature lands with its tests.
- R7.2 A state machine with two actors, retries or crash recovery has its rules as pure functions and tests for each rule.
- R7.3 Tests use no live network, no credentials and no real model. They read no real user settings: a test that depends on `~/.viv` is a bug (it once hung a suite).
- R7.4 Never skip, delete or loosen a test to pass.

## R8 Docs
- R8.1 `docs/architecture.md` shows the crate graph as a Mermaid diagram and matches `scripts/check-boundaries.sh`.
- R8.3 A structural change shows the architecture before and after as a Mermaid diagram in the PR.
