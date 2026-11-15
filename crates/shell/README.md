# vivianne-shell

The native app (`vivianne` binary). The only crate allowed to depend on a UI framework: GPUI Kit,
pinned to one exact version (ADR 0016). Kit brings its own GPUI snapshot, so import GPUI as `gpui_kit`.

Run offline with `VIV_DEMO=1`, or set `VIV_MODEL` plus a provider key (see `vivianne_providers::env`).
`src/bin/` holds throwaway spike binaries (see `docs/spikes.md`).

Windows release builds need `fxc.exe` from the Windows SDK; set `GPUI_FXC_PATH` if the build cannot find it.
Win32 calls need `unsafe`: only a dedicated platform module may `#[allow(unsafe_code)]`.
