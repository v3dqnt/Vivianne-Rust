# vivianne-telemetry

Local-first events, crash records and bug reports. Nothing leaves the computer unless the user turns sharing on
and sets an `https://` endpoint; both are off by default.

- `src/lib.rs`: the public functions (`track`, `install_panic_hook`, `read_events`, `read_crashes`, `summary`,
  `clear`, `submit_report`) and what the crate promises.
- `src/store.rs`: the files under `~/.viv/agent/` (`telemetry.jsonl`, `crashes/`): locked appends, size-based
  trimming, atomic crash files.
- `src/props.rs`: what an event may carry (numbers, booleans, short identifiers; never message content).
- `src/report.rs`: bug reports (always written to `~/.viv/agent/reports/` first) and the opt-in upload.

The files are the old app's, so existing events and crashes are read in place.
