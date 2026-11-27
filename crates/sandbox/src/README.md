# sandbox/src

- `env.rs`: the scrubbed environment every child starts from.
- `output.rs`: bounded output tails with private spill files.
- `tree.rs`: owning a process and everything it starts (job object or process group).
- `spawn.rs`: starting a program and terminating it; `spawn_tests.rs` runs real processes.
