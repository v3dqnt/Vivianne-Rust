# vivianne-shell

The native app (`vivianne` binary). The only crate allowed to depend on GPUI, pinned to one exact
version (ADR 0016). `main.rs` prints a version line for now; the real window comes after the Phase 1
spikes (see `docs/spikes.md`). Spike binaries live in `src/bin/`.

Windows release builds need `fxc.exe` from the Windows SDK; set `GPUI_FXC_PATH` if the build cannot find it.
Win32 calls need `unsafe`: only a dedicated platform module may `#[allow(unsafe_code)]`.
