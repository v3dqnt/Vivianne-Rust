# shell/src

- `main.rs`: starts the tokio runtime, registers Kit's icons, opens the window.
- `agent_setup.rs`: builds one agent session per thread (environment config for now).
- `theme.rs`: colour, radius and spacing tokens, pushed into Kit's theme.
- `demo.rs`: offline provider (`VIV_DEMO=1`) so the app can be run and checked without a key.
- `app/`: window, title bar with thread tabs, page switching.
- `chat/`: one chat (transcript and composer).
- `home/`: the Home page.
- `rail/`: Overview, Files, Preview and Terminal beside a chat.
- `settings/`: the Settings dialog.
- `bin/`: throwaway spike binaries.
