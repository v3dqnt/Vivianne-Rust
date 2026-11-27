# The original `sandbox` package group, and where Vivianne stands

The original is four packages (about 3 800 lines of source, 7 200 of tests): the confinement contract, per-platform
backends, the shared policy resolver, and the Windows write restriction. Code here: `crates/sandbox` (`policy`,
`confine`).

| Original | Vivianne | State |
|---|---|---|
| Modes `read-only`, `workspace-write`, `danger-full-access`; per-call policy | `policy::Mode`, `Policy` | **Ported.** |
| Writable roots; additional folders given by physical path only | `Policy::workspace_roots`, `writable_roots` | **Ported.** |
| One strictly wider mode per call, with a justification and approval | `tools::confinement`, `shell`, the agent loop | **Ported and wired:** offered only while a mode is standing; asks the user in every approval mode. |
| What the model is told | `Policy::describe`, `denial_marker`, `escalation_hint` | **Ported and wired** (a `sandbox-policy` prompt section while a mode is standing). |
| Standing default and per-session override | `VIV_SANDBOX`, `settings.json` `sandbox` | **Default only, wired.** No per-session switch and no control in the app's Settings. |
| Linux `bwrap` profile | `confine::bwrap_args` | **Ported, not run.** Argument lists are tested; no Linux machine has executed them. |
| Linux Landlock fallback | none | **Not ported** (needs its helper binary). |
| macOS Seatbelt profile | `confine::seatbelt_args` | **Ported, not run.** |
| **Windows write restriction** (restricted token, ACLs, Low integrity label) | none | **Not ported.** A confined mode on Windows fails with "unavailable"; it never runs unconfined. This is the largest gap. |
| Runner failure and denial classification | `confine::runner_failure`, `denied` | **Ported.** |
| Applied to the shell, filesystem and terminal tools | `shell`, `write`, `edit` | **Wired, partly.** Other tools that change files (`lsp_edit`, `convert_to_pdf`, `debug`, MCP tools, coding agents) are not held to the mode. The default is `danger-full-access`. On Windows a confined mode makes `shell` refuse. Do not treat Vivianne as sandboxed. |
