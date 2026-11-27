# sandbox/src

- `policy.rs`: the three file modes, writable folders, one-step escalation and the words the model is told.
- `confine.rs`: wrapping a command with `bwrap` or Seatbelt; refused where no backend runs.
- `env.rs`: the scrubbed environment every child starts from.
- `output.rs`: bounded output tails with private spill files.
- `tree.rs`: owning a process and everything it starts (job object or process group).
- `spawn.rs`: starting a program and terminating it; `spawn_tests.rs` runs real processes.
- `runner.rs`, `windows/`: the Windows confinement runner; `bin/` builds it alone for tests.
