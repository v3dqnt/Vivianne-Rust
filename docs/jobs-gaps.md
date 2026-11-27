# The original `jobs` packages, and where Vivianne stands

Original: `jobs/jobs`, `jobs/jobs-local`, `jobs/tool-jobs`, and the job half of `shell/tool-bash`. Here: `crates/tools/src/jobs/` and `crates/tools/src/simple/background.rs`.

| Original | State |
|---|---|
| Statuses, channels, views, producer handle and hooks | **Ported.** |
| Output ring at absolute offsets, 256 KiB live, 16 KiB settled, pull sources pumped every 150 ms | **Ported.** |
| Start, 10 live jobs per owner, ids, read, read-at, kill, wait, remove, first-wins settlement, teardown, events | **Ported.** |
| `job_output` (wait 30 s, cap 600 s), `job_list`, `job_kill`, the model's guidance | **Ported.** `job_output` and `job_list` do not ask; `job_kill` asks. |
| `run_in_background`; a command that outlasts its timeout becomes a job | **Ported** in `shell`. |
| Completion notices; a wait gives way when the person speaks | **Ported.** At most 10 runs in a row are started by jobs. |
| Whole output kept in a file past the memory window | **Ported:** `~/.viv/agent/tool-results/jobs/`, newest 20 kept, up to 64 MiB each. |
| Owners: one registry shared by every agent, fenced by session | **Narrowed.** Each toolbox has its own registry; the fencing is ported and tested but unused. Jobs end with their agent (another model chosen, app closed, end of a one-shot `viv` run or of a remote request). |
| Waking an idle agent | **App only.** `viv` hands notices to the next prompt; the remote server's agents live for one request. |
| Controllers, scoped event subscriptions, loader composition | **Not applicable:** the original's plugin host. |
| `job/registered` and `job/settled` session records, archive admission | **Not ported.** |
| Configurable limits, wait cap, wake cap | **Not ported:** fixed values. |
| Job rows in the UI, a person's own kill button | **Not ported.** A notice that arrives mid-run is not drawn in the transcript until the thread is reopened. |
| Jobs from other producers (workflows, terminals) | **Not ported:** only `shell` starts jobs. |

## Tests

The original's tests were **not** translated one to one. By its own files: `jobs` 8, `jobs-local` 102 (`jobs` 85, `pump` 6, `ring` 5, `events` 5, loader 1), `tool-jobs` 87. Here: 18 in `jobs/` (ring, registry, notices, tools) and 4 in `simple/background.rs` against real processes.

Covered here: ring eviction and UTF-8 tails; admission and the per-owner limit; owner isolation; read, read-at and the once-only result; kill and kill reasons; wait timeouts; event order and the awaited flag; teardown and drop; a failing pump source; notice wording and suppression; the three tools; the steered wait; foreground, background, promotion and abort of a real command.

Not covered here: event filtering by owner; a panicking listener or cancel; a producer that panics; output limits on notices and reads; id collisions; the wake cap; the app's idle wake (never run on screen).

Runs on Windows only; nothing here has run on Linux or macOS.
