# Projects: Phase 0 (groundwork)

The owner's proposal "Vivianne Public Projects & Collaborative Agent Development" (sections cited as §n) is the roadmap;
all its phases are intended. This file is its Phase 0: how it maps onto this repository, the vocabulary and state
machines, the threat model, and what must be decided before the later steps.

## 1. What exists and what does not

| The plan needs | Vivianne has | Gap |
|---|---|---|
| A person identity (§4, §10) | `multiplayer/src/identity.rs`: a key that authenticates a **machine** | No user accounts, sign-in or provider identity links. |
| A backend that authorizes every request (§4.2) | `crates/hub` (`viv-hub`), added with this work; `crates/remote` serves a person's own machines | Not deployed. One SQLite database, no outbox, no webhooks. |
| Projects (§3) | A folder. `multiplayer::state::Projects`/`Paired` calls a folder paired with one partner a "project" | The new `projects::Project` is a different thing; the pairing keeps its name and is untouched. |
| Tasks, threads (§9) | Saved threads (`runtime::store`) | Threads have no owner or visibility. |
| Isolated workspaces (§7) | `tools/src/git` finds and switches worktrees; `tools::workdir` keeps tools inside a folder | No workspace per run. No OS sandbox: `tools/src/code/sandbox.rs` strips credential variables and refuses a short list of destructive commands, and says itself it is not a jail. **§7.2 is not met.** |
| Repository integration (§6) | `tools/src/git/github.rs`: REST calls with the person's token (Settings, environment or `gh auth token`) | No GitHub App, installation tokens or webhooks. |
| Secrets (§7.2) | Model credentials in `ai` (file-backed store); credential variables are withheld from shells | No project secrets or secret references. |
| Permissions (§4) | `core::Mode` (Ask, Auto, Plan) and the tool lists | Governs one person's agent, not roles between people. |
| Audit (§10) | `projects::audit`, kept append-only by `hub` | Covers membership and project changes, not yet agent runs or repository actions. |

## 2. Resource hierarchy and permissions

`Project` contains repository connections, tasks and threads, agent runs, workspaces, change sets, artifacts and secret
references. Every permission is evaluated against a project and then the resource's own visibility. The vocabulary and
the role table live in one place, `crates/projects/src/permission.rs` (R1.6); anything not granted is denied.

An agent run is not its initiator: its permissions are the intersection of the human's, the project policy, the task
scope, the run grant and the platform limits (§4.4), and `core::Mode` still applies on top (R4.1, R4.3).

## 3. State machines

Defined as pure transitions with the clock passed in (R1.5, R7.2), each with tests:

- Project: `draft` to `active` to `archived`/`suspended`, then `deletion_pending` to `deleted` (§3.1).
- Join request: `pending` to `approved`/`rejected`/`withdrawn`/`expired`/`cancelled`, guarded by a state version (§5.1).
- Membership: `invited`, `active`, `suspended`, `removed` (§5.2).
- Repository access: the states of §5.3, separate from membership.
- Visibility is per resource; threads and agent traces are private by default (§3.2).

## 4. Threat model (summary of §13 against this repository)

| Threat | Control | Where |
|---|---|---|
| Acting on another project by changing an id | Every operation loads the actor's membership of that project and checks a named permission | `projects::service` |
| Self-escalation, removing the last owner, racing approvals | Rules in the role and membership machines; state versions | `projects::membership`, `join` |
| A removed member's queued run still acting | Re-authorize at start and before each side effect (§7.3) | `projects::agent` |
| Secrets in audit records | Redaction when the event is made | `projects::audit` |
| Prompt injection from repository text | Text is data, never authority (R4.4); policy comes only from the project record | everywhere |
| Malicious code run by a repository script | Needs a real sandbox. **Not available; block Phase 3 execution of untrusted setup until it is.** | open |

## 5. Audit event

Actor (person, agent run or system), action, project, resource, time, correlation id, policy version, and a small
metadata map with secret-looking keys and values removed. Appended, never edited by project members.

## 6. Repository provider

`RepositoryProvider` (§6.1) is the boundary; project, membership and task code never names GitHub. Adapters state
their capabilities explicitly. First adapter later: a GitHub App with least privilege. Expected permissions, to be
confirmed against GitHub's documentation when built: repository `metadata` read, `contents` and `pull requests` write,
`checks` read, and `administration` write only if collaborator invitations are synchronized.

## 7. Migration and rollback

Nothing existing changes format. A project record carries a `format` number; once records are saved, one from a newer
build must be refused and never rewritten (R3.5). `viv-hub` refuses a newer database; it does not yet check each record's `format`. Removing the feature is removing the crate and its files.

## 8. Where state lives

**Decided: a hosted Vivianne service** holds project state and enforces permissions. `crates/projects` has the rules;
`crates/hub` (`viv-hub`) stores them in SQLite and serves them over HTTP. It runs locally and is **not deployed**:
running it somewhere needs a host, TLS in front, backups, and a database that survives more than one node if it grows.

**Open: how people sign in.** Until then the operator issues tokens. GitHub sign-in would also give Phase 2 its linked
identities and needs an OAuth app registered by the owner; email links need a mail sender.

## 9. Phases and status

| Phase | Status |
|---|---|
| 0 Groundwork | This document. |
| 1 Identity and membership | Rules, SQLite storage, token sessions and the HTTP API for create, visibility, lifecycle, directory, join request/withdraw/decide, invite/accept, roles, suspend/remove/leave/block, ownership transfer and audit. Not built: sign-in, the client, `viv project`, app screens, reports, rate limits, platform suspension, notifications, deployment. Exit criteria are therefore **not** met. |
| 2 GitHub connection and sync | Not started (sync state machine only). |
| 3 Agent contribution | Not started (permission intersection only); needs a sandbox. |
| 4 Collaboration depth | Not started. |
| 5 Native hosting | Not started. |
| 6 Continuous sync research | Not started. |
