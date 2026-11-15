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
    },
    /// The step ceiling was hit before the model finished.
    StepLimit,
    /// The model answered without asking for tools.
    Done,
}
