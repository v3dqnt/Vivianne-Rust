# Porting the remaining DeepSeek Harness packages

Source: `deepseek-harness/packages`. Asked for: `acp`, `subagent`, `team`, `guard`, an "ACP v3" that joins the old Vivianne's ACP with DeepSeek's ideas, and after that every other package.

## What is already here

- **ACP client.** `crates/providers/src/acp` (about 6 900 lines) has everything the old `agent/harness.rs` (931 lines) did: registry with cache and fallback, ready marks, install, debug, trace, reset, start-failure hints, plan and tool-call mapping. There is nothing to copy across, and the app exposes debug, trace, reset and install under Settings, Providers. `viv` does not.
- **Subagents.** `crates/runtime/src/orchestration` has `task`, `task_output`, `wait_tasks`, `kill_task`, profiles and thread messaging; the thread tools are still unwired. Extend it; do not add a second registry (R1.6).

## What ACP v3 is

DeepSeek's `acp/acp` is an ACP **server**: other programs drive its agent. So v3 is:

1. the existing client, as it is;
2. **done:** `viv acp`, a stdio server over the agent loop in `crates/remote/src/acp` (gaps: `docs/acp-gaps.md`);
3. **done, narrowed:** `task` with `agent` delegates to a registry coding agent (`orchestration/coding.rs`; gaps: `docs/subagent-gaps.md`).

A permission request nobody answers is denied, never allowed (R4). Test the server by driving it with the existing client over an in-memory pipe.

## Order

| # | Group | Source lines | Notes |
|---|---|---|---|
| 1 | `guard` | 321 | Compare with `attempts.rs` and the agent's tool timeout first. |
| 2 | `subagent` | 9 900 | Gap table against `orchestration`; keep existing tool names where one exists. No unattended permission bypass for Claude Code or Codex by default. Check whether `subagent-acp` against `viv acp` covers `dsh-sdk`. |
| 3 | `team` | 1 340 | Needs 2. |
| 4 | `acp` server | 1 780 | Steps 2 and 3 of ACP v3. |
| 5 | everything else | several hundred thousand | One group at a time, each chosen with the user; much of it (`client`, `api`, `typert`, `test-support`, `boot`) belongs to the original's TypeScript host and has no counterpart. |

## Rules for each group

- The original's `describe` blocks are the working checklist; each group gets a `docs/<group>-gaps.md` written with the code, saying "partial" where it is.
- Files in `acp/{conn,install,mod,own_tools}.rs` carry another session's uncommitted edits: prefer new files, and never commit them.
- After any commit staged by hand, build `HEAD` alone in a worktree.
