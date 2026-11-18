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
    /// The step ceiling was hit before the model finished.
    StepLimit,
    /// The run was stopped by the user.
    Stopped,
    /// The model answered without asking for tools.
    Done,
}
