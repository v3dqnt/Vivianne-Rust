# tools/src/agent

Stand-ins for parts of the original app's `agent` module that the copied tools refer to by path
(`crate::agent::...`). See the crate README for the list.

- `orchestration.rs`, `thread_msgs.rs`: no running thread yet.
- `system/`: where files live under `~/.viv`.
- `runtime/`: an atomic file write, and the process-tree helpers (copied).
- `tools/`: the settings file reader.
