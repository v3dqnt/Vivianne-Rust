# vivianne-shell

The native app (`vivianne` binary). It draws and reads input and only wires the other crates (R1.2, R1.3); the state it owns is about the window. It is the only crate that depends on a UI framework: GPUI Kit, pinned to one exact version (ADR 0016), imported as `gpui_kit`, with a vendored, patched `gpui-component` (see `vendor/`).

## Layout

| Folder or file | What it is |
|---|---|
| `main.rs`, `backend.rs`, `agent_setup.rs` | Start-up: the window (blurred backdrop), the background services, a thread's agent |
| `app/` | The root view: title bar with thread tabs, pages, rail toggle |
| `home/` | Projects, threads and the Machines page |
| `chat/` | Transcript, composer, tool-call cards, model picker (including an agent's own model) |
| `rail/` | The side panel: Overview, Files, Preview, Terminal, Debug (DAP debugger) |
| `settings/` | The Settings dialog and its pages: Account and usage, Providers, Connections, Machines, Characters, and the rest |
| `pair/`, `git/`, `signin/` | Pairing with a partner, the repository page, sign-in windows |
| `brand.rs`, `characters/` | App and provider icons (local file, fetched site icon, bundled mark, letter) and the mini characters, drawn by a small software renderer |
| `theme.rs`, `assets.rs` | Colours, radii, sizes; embedded fonts (Allo), logos and models |
| `notify.rs`, `perf.rs`, `ui_state.rs` | Notifications, the frame-time meter, remembered UI state |

Run offline with `VIV_DEMO=1`, or set `VIV_MODEL` plus a provider key (see `vivianne_providers::env`). `src/bin/` holds throwaway spike binaries (`docs/spikes.md`).

Windows release builds need `fxc.exe` from the Windows SDK; set `GPUI_FXC_PATH` if the build cannot find it. Win32 calls need `unsafe`: only a dedicated platform module may `#[allow(unsafe_code)]`. Judge smoothness only in a release build (`cargo build --release -p vivianne-shell`).

Nothing on the render path waits on the network, a child process, a model or slow storage (R2.1): slow work goes through `backend::run` or `backend::spawn`.

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `memory`, `multiplayer`, `remote`, `core`, `ai`, `telemetry`, `providers`, `runtime`, `tools`, `attachments`.
- **State it owns** (R1.6): only what is about the window: the remembered rail tab and collapsed sections (`agent/viv-ui-state.json`), the profile picture and banner (`profile/`), rendered character frames (`characters/`) and fetched app icons (`icons/`).
- **Check it:** `cargo test -p vivianne-shell` and `cargo clippy -p vivianne-shell --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
