# runtime/src/schedule

Scheduled tasks: instructions delivered to a conversation at a chosen time or on a repeating schedule. Port of the
original `schedule` and `tool-schedule`; the gaps are in `docs/schedule-gaps.md`.

- `domain.rs`: the task record, how a schedule is written and checked, and when it next fires (daily and weekly in a
  named time zone, cron, every, once). Pure: the clock is passed in.
- `store.rs`: tasks kept in `~/.viv/agent/schedules.json`, with the rules for what a restart delivers.
- `tools.rs`: the model-facing `schedule_create`, `schedule_list`, `schedule_view` and `schedule_edit`.
