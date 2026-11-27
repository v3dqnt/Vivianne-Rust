# projects/src

- `model.rs`: ids, the project record, visibility, lifecycle, join policy and per-resource sharing.
- `permission.rs`: roles, permissions and the one `authorize` check. Deny by default.
- `membership.rs`: membership states, role-change and removal rules, ownership transfer.
- `join.rs`: join requests and who may make one.
- `agent.rs`: what an agent run may do, always less than or equal to its initiator.
- `sync.rs`: repository-access states, kept apart from membership.
- `audit.rs`: audit events, with secrets taken out.
- `provider.rs`: the boundary a repository host is behind.
- `store.rs`: the `Store` trait. `memory.rs`: the in-memory one.
- `scenarios.rs`: the acceptance scenarios every backend runs (feature `scenarios`).
- `service.rs`, `service_members.rs`, `service_reads.rs`: the operations; each loads the actor's membership and checks it before acting.
