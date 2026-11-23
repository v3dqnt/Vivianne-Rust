# Plan: the harness that improves itself

Goal: every session becomes data. A module reads that data, finds where the model struggles inside Vivianne's
harness, and tunes the harness so the same model does better next time. The user stays in charge: nothing that
changes behaviour is applied without evidence, and everything is one click from being undone.

Status: plan only, nothing built. Written 2026-11-21.

## What "the harness" is, concretely

The harness is everything around the model that Vivianne controls. Those are the knobs, and the only things this
module may touch:

| Knob | Where it lives today |
|---|---|
| System prompt sections and per-model addenda | `runtime/src/prompt.rs` (`coding_system_prompt`) |
| Tool names, descriptions, parameter docs | `tools/src/schemas.rs` |
| Which tools each mode offers | `core` (`Mode`, `LOOK_ONLY_TOOLS`) |
| Tool-result handling: truncation, masking of old results | `runtime/src/history.rs` (`mask_old_results`) |
| Compaction: when (80%), how much is kept (20%), where it cuts | `runtime/src/compact.rs` |
| Step ceiling, retry policy, fallbacks | `runtime/src/agent.rs`, `resilience/` |
| Project instructions: how AGENTS.md / CLAUDE.md is injected | `runtime/src/instructions.rs` |
| Memory recall budget and what is injected each turn | `memory/src/agent.rs` |
| Default thinking level per kind of task | `shell/src/chat/composer` |

**Never tuned, by construction:** permission modes and approval gates, the file guard and sandbox, secret
handling, anything that decides what the agent may do rather than how well it does it. The module has no write
path to those.

## The loop

```
 record ──► label ──► analyse ──► propose ──► validate ──► promote
   ▲                                                          │
   └──────────── new harness version, stamped on every run ◄──┘
```

### 1. Record: a trace per run

Every run writes one file under `~/.viv/lab/traces/<thread>/<run>.jsonl`, fed by the `AgentEvent` stream the
shell already consumes (`ToolStart`, `ToolEnd`, `Approval`, `Retrying`, `Fallback`, `Usage`, `Compacted`,
`StepLimit`, `Stopped`, `Done`). Each line is timestamped. Each run header carries:

- the **harness version id** that was active (a hash of the knob values), the model, the provider, the mode;
- the user's request and the project's folder (hashed unless the project is opted in to content);
- what happened after: the checkpoint **undo** if used, whether the changed files were later reverted in git, the
  next user message (a correction? a thanks? a new topic?), whether the thread ended or was abandoned.

Tool outputs are stored truncated and with secrets stripped (reuse `memory::defense`). Content never leaves the
machine. Telemetry's rule stands: counts and identifiers only go anywhere else.

### 2. Label: outcomes without asking the user

Most sessions will never get a thumbs up, so outcomes come from signals:

- **Bad:** undo used; a later turn restates the same request; "no", "that's wrong", "not that"; approval denied;
  the same edit retried three times; step limit hit; compaction followed by the model re-reading what it just
  had; the changed files reverted in git within a day.
- **Good:** tests or the build ran green after the last edit; the user moved on without correcting; the change was
  committed and kept.
- **Explicit:** an optional thumbs up/down on a reply, one click, never asked for.

A **judge pass** (the user's own configured model, run while idle, same scheduler as memory's sleep-time work)
reads a trace and writes a short structured verdict: task, outcome, the first point things went wrong, and a
failure label from a growing taxonomy. Rule-based detectors run first and for free; the judge only reads traces
the detectors flagged or sampled ones for calibration.

### 3. Analyse: what keeps going wrong

Per harness version and per model: success proxy, tokens and cost per task, steps per task, tool error rate by
tool, retries, compactions, time to first edit. Then failure clusters, each a sentence plus links to the traces
behind it, for example:

- "Edit fails with 'old_string not unique' in 18% of edits: the tool description does not say to include context."
- "The model re-reads the same file 3+ times after compaction on long threads."
- "Model X ignores AGENTS.md when it appears after the tool list."

### 4. Propose: one small change with a hypothesis

An **optimizer** model reads one cluster plus the current knob values and proposes **one** change: a diff to a
prompt section, a tool description, or a number, with the hypothesis and the metric it should move. Rules:

- One change at a time, so a result can be attributed.
- Proposals are text diffs against the version store, never code.
- Tool outputs in traces reach the optimizer as untrusted data (they can contain prompt injection).
- The optimizer's own prompt is a harness knob too, but it is tuned only with explicit approval (see "Recursive").

### 5. Validate: evidence before anything ships

Three tiers, cheapest first. A proposal must pass the one that matches its risk.

1. **Replay on frozen traces.** Re-run the model on recorded runs up to the failure point with the candidate
   harness and check whether it avoids the mistake. Cheap, good at catching "this prompt made it worse", weak
   at proving it got better (the model diverges after the first step).
2. **A task suite from the user's own history.** Past tasks that have a verifiable end (a test that failed and
   then passed, a build that was broken and fixed) are turned into evals: a git worktree at the "before"
   commit, the original request, a check command. Candidate and baseline each run N times with a cheap model;
   the comparison is paired and needs a minimum sample before it counts.
3. **Shadow A/B on live use**, opt-in: a share of new threads (default 20%) start on the candidate. Compared on
   the same proxies after enough runs. Only for low-risk knobs (descriptions, budgets), never for anything
   that changes what the agent may do.

### 6. Promote: the user stays in control

Autonomy is a setting, default **Suggest**:

| Level | What happens |
|---|---|
| Off | Nothing recorded. |
| Observe | Traces and metrics only. |
| **Suggest** (default) | Proposals appear as cards with the diff, the evidence and the traces; the user accepts, rejects or "tries longer". |
| Auto-apply low risk | Description and budget changes that passed tier 2 or 3 apply themselves, with a notice. |

Every accepted change becomes a new harness version in `~/.viv/lab/harness/<id>.json`. The previous one is kept.
**Rollback is one click**, and an auto-rollback fires if the live proxies regress beyond a threshold after a
promotion.

## Where it lives

- `crates/lab` (new): trace types and writer, signal detectors, metrics, the version store, the proposal
  records, the eval runner. No UI, no model calls of its own; it takes a completion function from the caller,
  like `memory` does.
- `runtime`: reads the active harness version when building an agent; stamps it on every run. A `Harness`
  value replaces the constants now spread across `prompt.rs`, `compact.rs`, `history.rs`.
- `shell`: a recorder subscriber beside the one that applies events to the transcript; a **Lab** page (Settings
  or its own tab) showing versions, metrics, clusters, proposals and rollback. The old app's Trajectory view
  (`gui-next/src/modules/trajectory`) is the model for the per-run timeline.
- Idle scheduling reuses memory's sleep-time scheduler so analysis never competes with a live turn.

## Phases

| Phase | Ships | Behaviour change |
|---|---|---|
| **0. Plumbing** | `Harness` value with today's defaults; version id stamped on runs; trace recorder. | None |
| **1. Look** | Rule-based detectors, metrics per version and model, a read-only Lab page with run timelines. | None |
| **2. Understand** | Judge pass and failure clusters. Proposals written but only shown. | None |
| **3. Prove** | Eval suite built from the user's history; worktree runner; candidate vs baseline report. | None |
| **4. Apply** | Proposal cards, version store, promotion, rollback, shadow A/B. | Opt-in |
| **5. Per-model** | Separate harness profiles per model; the optimizer's own prompt becomes tunable (approval only). | Opt-in |

Phases 0 to 2 are safe to ship without changing how the app behaves, and are enough to be useful on their own:
the user can see where sessions go wrong.

## Risks and how the design answers them

- **Optimising a proxy (Goodhart).** Undo-rate can be gamed by making the model timid. Several independent
  signals, plus the paired eval with a real check command, plus "success proxy" never shown as a single score.
- **Noise and small samples.** Minimum run counts, paired comparisons, no promotion on a hunch.
- **Drift when the model changes.** Versions and metrics are keyed by model; a model update starts a fresh
  baseline.
- **Overfitting to one user.** Fine by intent (it is their harness), but proposals that only help one project's
  quirks go into that project's instructions, not the global harness.
- **Privacy.** Local only; content capture is opt-in per project; one command deletes all traces; analysis
  uses the model the user already configured, or a local one.
- **Self-modification safety.** The tunable surface is a closed list; no code, no permissions, no sandbox.
  The optimizer's output is validated against that list before it is even shown.
- **Prompt bloat.** Every proposal states its token cost; the optimizer is told to prefer deleting text, and a
  size budget caps the system prompt.
- **Cost.** A monthly cap on judge, optimizer and eval spend, shown on the Lab page.

## Decisions needed before phase 0

1. **Default autonomy.** Recommended: Suggest. Auto-apply only after the user turns it on.
2. **Whose model reads the traces.** Recommended: the model the user already uses for their threads, with a
   local-model option, and nothing sent anywhere else.
3. **Scope.** Recommended: tune Vivianne's own agent loop first. Coding agents run over ACP (Claude Code and
   the like) have their own harness; for them the module only observes (what they ask, what was undone), and
   the only knob is what Vivianne's own tools and instructions offer them.
4. **Content capture.** Recommended: off until a project is opted in; counts-only metrics work without it, but
   the judge and the evals need content.
