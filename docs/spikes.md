# Phase 1 spikes

Run on the dev machine (Windows 11, release build, GPUI 0.2.2). Code in `crates/shell/src/bin/`.

## (a) Streaming transcript: PASS on pacing, selection not tested

200+ text blocks plus code blocks, ~1500 chars/s appended to the last block, auto-scroll to bottom,
whole tree re-rendered every frame, no virtualization.

| Run | Result |
|---|---|
| Timer-driven (16 ms timer) | 33 fps. Misleading: the Windows timer rounds up to ~31 ms. |
| Frame-driven (`request_animation_frame`) | 1144 frames in 8 s, avg gap 7.0 ms, p95 7.8 ms, max 43 ms (~143 fps, display refresh) |

Lesson: drive streaming updates from the frame loop or batch them; do not rely on short timers on Windows.

Not tested: text selection across blocks (GPUI has no selectable text out of the box; we will need our own
selection layer), markdown layout, syntax-coloured code, images. Those are Phase 2 work and the main
remaining risk for the go/no-go gate.

## (b) Click-through overlay: PASS (by OS query), not yet looked at

Transparent, borderless popup window; `WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW |
WS_EX_NOACTIVATE` applied through the raw HWND. The OS reports `click_through=true topmost=true layered=true`,
and `WindowFromPoint` at the window centre does not return the overlay (`hit_test_sees_overlay=false`), so
clicks pass through.

Not verified: that the window looks right (translucent blue dot, no opaque background) and that several
overlays can coexist. Needs a screenshot.

Policy: this needs `unsafe`. The workspace lint is `deny`; only a dedicated platform module in the shell may
`#[allow(unsafe_code)]`.

## (c) Terminal: PASS

`alacritty_terminal` 0.25 drives a Windows pseudo terminal (ConPTY) and keeps a correct screen grid:
a three-command line produced the expected lines, with older lines scrolling off a 12-row grid as they
should. One earlier run of `echo ... && ver` did not show the `ver` output; not reproduced or explained.
Drawing the grid is plain text drawing, which (a) shows is fast enough. Input, resize, colours and
scrollback are not exercised yet.

## Build note

GPUI release builds on Windows need `fxc.exe` (Windows SDK). If the build says "Failed to find fxc.exe", set
`GPUI_FXC_PATH` to e.g. `C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\fxc.exe`.
