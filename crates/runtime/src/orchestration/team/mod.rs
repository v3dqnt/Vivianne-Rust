//! A task board shared by one conversation's agent (the lead) and its subagents.
//!
//! Port of `@deepseek-ai/dsh-agent-team` and `dsh-tool-agent-team`. Gaps: `docs/team-gaps.md`.

mod board;
mod store;
mod tools;

#[cfg(test)]
mod tests;

pub use board::{Action, Actor, Create, Status, Task, TeamError, Update, View};
pub use store::Store;
pub use tools::{Kind, TeamTool};

/// Told to every agent that has the board's tools.
pub const POLICY: &str = "A shared task board is available in this conversation: the team_task_* tools. The lead (this \
conversation's own agent) and its subagents see the same board. Task owners are `lead` or a subagent's name. The lead \
can reassign a task to itself or to a subagent that is still working; others claim ready tasks themselves.\n\n\
Split write work into disjoint parts, record the paths a task expects to change in write_scopes, and use blocked_by \
when work must be ordered. Overlapping write scopes only warn; nothing is locked.\n\n\
The workflow is list, get, claim with the current revision, do the work, then complete. A ready task starts nobody: \
the lead starts a subagent with `task` and tells it which task to claim, then collects it with task_output or \
wait_tasks, and does not give its final answer while it still needs one. A subagent that finishes or is stopped does \
not give up its tasks: the lead releases them.";
