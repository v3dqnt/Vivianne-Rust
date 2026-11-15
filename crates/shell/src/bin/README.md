# shell/src/bin

Throwaway Phase 1 spike binaries. Each one proves a single risk; findings are in `docs/spikes.md`.
Delete or promote into the real shell once their job is done.

- `spike_overlay.rs`: transparent, always-on-top, click-through window; asks the OS to confirm.
- `spike_terminal.rs`: `alacritty_terminal` driving a Windows pseudo terminal.

`spike_overlay.rs` is the only file allowed `unsafe` (Win32 calls); the workspace lint is `deny` so each
opt-in is explicit.
