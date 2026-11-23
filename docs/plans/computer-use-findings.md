# Computer use on agent-desktop: Phase 0 findings (2026-11-21)

Tried `agent-desktop` v1.3.1 (Windows x86_64 build from the project's GitHub release, SHA-256 checked against the
release's published digest) on this machine, by hand and over its MCP server. Nothing was installed system-wide: the
binary ran from a scratch folder. This is what the plan in `~/.claude/plans/mutable-frolicking-flask.md` needed to
know before building.

## Works

- **Windows coverage is good.** Accessibility permission reads as granted with no setup. Supported surfaces include
  window, focused, menu, start menu, taskbar, system tray and action center.
- **Snapshots are small and rich.** Notepad (4 tabs open): 30 refs, 7 KB, with tab names, their modified state and
  the text editor's value. Calculator (a Store app): 40 refs, 10 KB, every button named. Roughly 2-3k tokens each.
- **Acting works, headless.** In Calculator, `click` on the refs for 5, +, 3, = left the display at "Display is 8", with
  no pointer movement. The window's own Close button, found by name in a snapshot, closed it.
- **MCP server:** `agent-desktop mcp` speaks MCP over stdio and lists **75 tools** with full input schemas
  (`desktop_snapshot`, `desktop_click`, ...). All 75 are about 11k tokens of tool definitions; a curated set of 26
  (observe, act on a ref, keys, launch/close, windows, wait) is about 4.6k.
- **Envelope:** every result is JSON with `ok`, `command`, `data` or `error`, and `disposition` (`delivery`,
  `retry`: `safe` or `unsafe`). A click reports `delivered_unverified` and `retry: unsafe`, so the agent must
  re-snapshot to verify, as the project says.

## Surprises that shape the design

1. **No read-only marking.** Tools carry no annotations, so Vivianne needs its own list of which tools only look.
2. **Too many tools.** 75 tools is a lot of context and mostly irrelevant (sandboxes, notifications, cursor overlay,
   sessions). Vivianne should offer a curated subset: needs a per-server tool allowlist in the MCP client.
3. **It sees everything.** A snapshot of Notepad included my unsaved text; `list-windows` listed every open window's
   title, including a browser tab's page title. That is the feature, and also why screenshots, clipboard reads,
   window lists and snapshots must never be saved into threads, the ledger or telemetry, and why Ask mode must gate
   them.
4. **Attached to my existing Notepad.** `launch Notepad` did not start a new one; it attached to the open one with
   the user's tabs. The agent must never type into an app it did not open without approval.
5. **Bare launch names are restricted** to System32 and the Windows directory: `launch Calculator` fails,
   `launch calc.exe` works. Store apps cannot be closed by name (`close-app` needs exact process identity); closing
   goes through the window's Close button.
6. **One intermittent failure:** a snapshot failed once with `INTERNAL ... atomic replace failed ... os error 1175`
   (its state file under `~/.agent-desktop`). The immediate retry worked. The wrapper retries once when the error says
   `retry: safe`.
7. **Physical input is opt-in:** `hover`, `drag`, `double-click`, `mouse-*` need `--headed`; `key-down/up` and
   `mouse-down/up` always refuse. Vivianne never passes `--headed` by default.

## Decisions this settles

- Run it as a managed MCP server, as planned. No need for the C library.
- Get the binary from the GitHub release (about 1.8 MB), checksum verified, into `~/.viv/bin`.
- Offer the model a curated tool subset, not all 75.
- Gate by Vivianne's own list: look-only = `snapshot find get is list_* status wait`; sensitive = `screenshot
  clipboard_get list_notifications` (approval in Ask, never stored); everything else changes things.
