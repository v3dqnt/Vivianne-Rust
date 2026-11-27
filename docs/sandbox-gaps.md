# The original `sandbox` package group, and where Vivianne stands

The original is four packages (about 3 800 lines of source, 7 200 of tests): the confinement contract, per-platform
backends, the shared policy resolver, and the Windows write restriction. Code here: `crates/sandbox` (`policy`,
`confine`).

| Original | Vivianne | State |
|---|---|---|
| Modes `read-only`, `workspace-write`, `danger-full-access`; per-call policy | `policy::Mode`, `Policy` | **Ported.** |
| Writable roots; additional folders given by physical path only | `Policy::workspace_roots`, `writable_roots` | **Ported.** |
| One strictly wider mode per call, with a justification and approval | `policy::check_escalation_args`, `escalate` | **Ported** as rules; no tool offers `sandbox_permissions` yet. |
| What the model is told | `Policy::describe`, `denial_marker`, `escalation_hint` | **Ported** as text; nothing adds it to a prompt yet. |
| Standing default and per-session override | `policy::standing_mode` | **Rule only.** No setting or session switch stores a mode. |
| Linux `bwrap` profile | `confine::bwrap_args` | **Ported, not run.** Argument lists are tested; no Linux machine has executed them. |
| Linux Landlock fallback | none | **Not ported** (needs its helper binary). |
| macOS Seatbelt profile | `confine::seatbelt_args` | **Ported, not run.** |
| **Windows write restriction** (restricted token, ACLs, Low integrity label) | none | **Not ported.** A confined mode on Windows fails with "unavailable"; it never runs unconfined. This is the largest gap. |
| Runner failure and denial classification | `confine::runner_failure`, `denied` | **Ported.** |
| Applied to the shell, filesystem and terminal tools | nothing yet | **Not wired.** Today's `shell` tool runs as before, with the credential scrub and destructive-command refusals in `tools/src/code/sandbox.rs`. Do not treat Vivianne as sandboxed. |
