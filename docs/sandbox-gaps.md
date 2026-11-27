# The original `sandbox` package group, and where Vivianne stands

The original is four packages (about 3 800 lines of source, 7 200 of tests): the confinement contract, per-platform
backends, the shared policy resolver, and the Windows write restriction. Code here: `crates/sandbox` (`policy`,
`confine`, `windows`).

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
| Windows write restriction (restricted token, ACLs, Low integrity label) | `crates/sandbox/src/windows` | **Ported, narrowed, run on Windows 11.** The program is its own runner. Writes and deletes only; the grant on a project folder is permanent (see that folder's README). A project that holds the running Vivianne build is refused. |
| Runner failure and denial classification | `confine::runner_failure`, `denied` | **Ported.** |
| Applied to the shell, filesystem and terminal tools | `shell`, `write`, `edit` | **Wired, partly.** Other tools that change files (`lsp_edit`, `convert_to_pdf`, `debug`, MCP tools, coding agents) are not held to the mode. The default is `danger-full-access`. The remote server cannot confine on Windows unless the program hosting it is `viv` or the app. Do not treat Vivianne as sandboxed. |

## Windows: not carried over

- The seam-managed mode (the host grants once and passes `--write-sid`); every confined command checks and, if needed, makes the grant itself, with a fresh private temporary folder each time.
- Revoking a grant, the ACL diagnosis skill and its repair script, the control pipe on file descriptor 7, a runner-side warning for large trees or FAT volumes.

## Tests

The original's tests (`sandbox` 35, `sandbox-local` 80, `sandbox-policy` 20, `sandbox-windows-acl` 190) were **not** translated one to one. Here: policy and argument tests in `policy_tests.rs` and `confine.rs`, quoting, SIDs, refused folders and arguments in `windows/`, and four tests with real children in `tests/windows.rs` (write inside only, delete outside refused, read-only writes nothing while an unconfined control does, a grandchild held, a refused folder runs nothing). Not covered: Win32 failure paths, exact ACE inspection, concurrent grants.
