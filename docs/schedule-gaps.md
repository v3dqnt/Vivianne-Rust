# The original `schedule` package group, and where Vivianne stands

The original is two packages (about 4 000 lines of source, 7 900 of tests): a host service that stores tasks, fires
them and delivers them to a Session, and four model-facing tools. Code here: `crates/runtime/src/schedule/`.
Vivianne's older `automation` (cron prompts and folder watching, with steps) stays beside it.

| Original | Vivianne | State |
|---|---|---|
| `after`, `at` (instant or local date/time), `every` (at least 60 s), `daily`, `weekly`, `cron` | `schedule::domain` | **Ported.** Named time zones; a local time that does not exist is skipped (refused for `at`), one that happens twice fires the first time. |
| `rrule` timing | none | **Not ported.** No RFC 5545 engine in the tree. |
| Latest missed occurrence only after a restart | `domain::resolve` | **Ported.** |
| Reminder framing, batch framing | `domain::reminder`, `reminder_batch` | **Ported.** The app sends one message per task; the batch form is not used yet. |
| Titles required, error codes | `domain::ScheduleError` | **Ported:** the same codes. |
| Targets: a session, `new_once`, `new` with cwd and agent preset | `domain::Target`, app delivery | **Ported, narrowed.** The app opens the named thread (a new one if it is gone) and waits while it is busy. The create spec's folder and profile are stored but a new thread uses the open project's. |
| Durable storage, optimistic edits, pause, delivery history, catalog paging | `schedule::store` (`~/.viv/agent/schedules.json`) | **Ported.** File lock, `expected_updated_at`, 20 deliveries per task, 200 tasks. A task is marked delivered after it is sent. Resuming a paused task does not send its backlog. |
| Delivery history pages with cursors, retention settings, delete-inactive, run-now | none | **Not ported.** |
| `schedule_create` / `list` / `view` / `edit`, refused for subagents | `schedule::tools` | **Ported.** Create and edit ask for approval; list and view do not. |
| Host service firing without a window; archive admission; protocol/UI pages | the app's 30-second timer | **Narrowed.** Tasks fire only while the app is open (the original needs its Host running too). `viv` and the remote server do not fire them. There is no Settings page for tasks yet. |
| Also fixed here | `automation` | Five-field cron weekdays were read with the `cron` crate's numbering (1 = Sunday), so weekday automations ran a day early. They now use standard cron. |

The delivery path in the app has not been exercised by a test or by hand.
