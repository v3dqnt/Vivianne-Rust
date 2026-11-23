# shell/src

- `main.rs`: starts the tokio runtime, registers Kit's icons, opens the window.
- `agent_setup.rs`: builds one agent session per thread (environment config for now).
- `theme.rs`: colour, radius and spacing tokens, pushed into Kit's theme.
- `demo.rs`: offline provider (`VIV_DEMO=1`) so the app can be run and checked without a key.
- `app/`: window, title bar with thread tabs, page switching.
- `chat/`: one chat (transcript and composer).
- `git/`: the Git tab: changes with diffs, pull requests, Actions, issues, releases, history.
- `home/`: the Home page.
- `rail/`: Overview, Files, Preview and Terminal beside a chat.
- `settings/`: the Settings dialog.
- `bin/`: throwaway spike binaries.
- `backend.rs`: the key store and coding-agent manager opened at start, and `run` to do async work from the UI.

- `notify.rs`: desktop notifications when the window is not in front: a toast with Approve and Reject for something
  waiting on you, and a toast with a reply box for a finished reply (only when `notifications.enabled` is true in
  `~/.viv/agent/settings.json`, as in the old app). Windows only; elsewhere nothing is shown.
- `perf.rs`: the pace of the whole window, recorded by the root view on every frame; the Debug tab reads it.
