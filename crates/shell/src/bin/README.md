# shell/src/bin

Throwaway Phase 1 spike binaries. Each one measures or proves a single risk; findings are written to
`docs/spikes.md`, then the binary is deleted or promoted into the real shell.

- `spike_transcript.rs`: (a) streaming long transcript, prints frame-gap statistics.
- `spike_overlay.rs`: (b) transparent, always-on-top, click-through window; asks the OS to confirm.
- `spike_terminal.rs`: (c) `alacritty_terminal` driving a Windows pseudo terminal.

`spike_overlay.rs` is the only file allowed `unsafe` (Win32 calls); the workspace lint is `deny` so each
opt-in is explicit.
