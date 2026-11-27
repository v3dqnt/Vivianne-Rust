# jobs

Background work that outlives the tool call that started it. Port of the original `jobs`, `jobs-local` and `tool-jobs` packages. It lives in `tools` because `shell` starts jobs and `tools` cannot depend on `runtime`.

- `types.rs`: statuses, channels, views, events, the producer's handle, hooks and spec.
- `ring.rs`: the bounded output of one job, at offsets that never move.
- `job.rs`: one job: settlement (first wins), kill, teardown, waiters.
- `registry.rs`: admission (10 live per owner), ownership, reads, wait, remove, teardown.
- `events.rs`: subscriptions.
- `pump.rs`: drains a job's pull sources into its ring.
- `render.rs`: status lines, output deltas, the completion notice.
- `tools.rs`: `job_output`, `job_list`, `job_kill`.
- `mod.rs`: `Jobs`, one toolbox's registry, and the model's guidance.

A toolbox owns its jobs: dropping it (another model chosen, the app or `viv` closing) cancels them.
