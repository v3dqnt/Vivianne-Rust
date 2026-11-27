# The original `terminal` packages, and where Vivianne stands

Original: `terminal/terminal`, `terminal/terminal-bash`, `experimental/tool-terminal`. Here: `crates/tools/src/terminal/`. The app's own terminal panel is separate and unchanged.

| Original | State |
|---|---|
| `terminal_open`, `send`, `read`, `signal`, `close`, `list` | **Ported.** Open, send, signal and close ask; read and list do not. |
| Owner isolation and teardown | **Ported by construction:** each toolbox has its own table (8 sessions), ended with it. |
| One send at a time; `run_in_background` as a job | **Ported.** |
| Waiting: session exit, timeout (30 s), inferred idle (3 s of silence) | **Ported.** |
| Waiting: `stdin_read` from process inspectors, prompt detection | **Not ported.** Every wait at a prompt takes the 3 s of silence. A program silent for longer than that while working reads as idle. |
| Escape-sequence sanitiser and scrollback | **Replaced** by the `alacritty_terminal` emulator: 160x40, 10 000 lines, 256 KiB a read. |
| Signals to the foreground process group | **Narrowed:** `SIGINT` (Ctrl+C) everywhere, `SIGTSTP` on Unix; `SIGTERM`, `SIGHUP`, `SIGKILL` are refused. |
| Bash or PowerShell dialect, shell path and limits configurable | **Not ported:** PowerShell on Windows, `bash` elsewhere, fixed limits. |
| Sandbox mode applied to the shell | **Ported:** wrapped like `shell`, refused where it cannot be. |
| Scrubbed environment | **Narrowed:** secrets are set to empty strings rather than removed. |
| Closing waits until the process tree is gone | **Not ported.** Closing asks the shell to exit and closes the pseudo terminal; a program it detached can survive. |
| A person's view of the agent's terminals | **Not ported.** |

## Tests

The original's tests were **not** translated. Here: four against a real PowerShell on a real pseudo terminal (state across sends, exit, refused requests, refused mode) and one run by hand for a confined `read-only` terminal. Not covered: timeout, background send, signals, concurrent sends. Windows only; never run on Linux or macOS.
