//! What the loop tells its caller while it works.

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A piece of the assistant's reply.
    Text(String),
    ToolStart {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolEnd {
        id: String,
        name: String,
        output: String,
        is_error: bool,
        /// What the tool tells the UI beyond its text (the original tools' `details`).
        details: Option<serde_json::Value>,
        /// The call's arguments, when they are only known now (a coding agent's edit knows its before and
        /// after at the end). `None` leaves what the start said.
        args: Option<serde_json::Value>,
    },
    /// Something a coding agent wants to do and asks first. The answer goes back through whoever holds the
    /// request, by `id`.
    Approval {
        id: String,
        title: String,
    },
    /// A model call failed for a passing reason and will be made again after `delay`.
    Retrying {
        attempt: u32,
        max_attempts: u32,
        delay: std::time::Duration,
        reason: String,
    },
    /// A model could not answer, so the next one in line is being used.
    Fallback {
        from: String,
        to: String,
        reason: String,
    },
    /// What one model call used, sent when it ends (before the call's tool results). The loop does not know
    /// which thread it runs for, so whoever does records it.
    Usage {
        provider: String,
        model: String,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        /// `None` when the model has no known price: unknown, never zero.
        cost: Option<f64>,
    },
    /// The step ceiling was hit before the model finished.
    StepLimit,
    /// The run was stopped by the user.
    Stopped,
    /// The model answered without asking for tools.
    Done,
}
