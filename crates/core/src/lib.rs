//! Domain types shared by every crate: threads, messages, tool calls, ids.
//! No I/O and no async here; this crate stays dependency-light on purpose.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable identifier of a conversation thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// How much the model should think before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl Thinking {
    pub const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    /// The level's name as providers spell reasoning effort (`low`, `medium`, `high`);
    /// `None` when thinking is off.
    #[must_use]
    pub fn effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
        }
    }
}

/// How much the agent may do on its own: ask before it changes anything, act freely, or only plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Pauses before a tool that changes things, so the user approves each one.
    #[default]
    Ask,
    /// Acts without asking.
    Auto,
    /// Researches and proposes a plan. Changes nothing.
    Plan,
}

/// Tools that only look. Anything not named here (a new tool, an editing one such as `lsp_edit`) counts as one that
/// changes things: a list of what is safe fails closed, a list of what is dangerous fails open.
const LOOK_ONLY_TOOLS: [&str; 12] =
    ["read", "ls", "grep", "code_search", "glob", "diagnostics", "lsp", "repo_overview", "recall", "reflect", "list_threads", "read_thread"];

impl Mode {
    pub const ALL: [Self; 3] = [Self::Ask, Self::Auto, Self::Plan];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask for approval",
            Self::Auto => "Auto",
            Self::Plan => "Plan",
        }
    }

    /// One line for the menu.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            Self::Ask => "Pauses before a tool that changes things, so you approve each one.",
            Self::Auto => "Acts without asking. Files and commands run as the agent decides.",
            Self::Plan => "Researches and proposes a plan for review. Changes nothing until you approve it.",
        }
    }

    /// The instruction added to the system prompt while this mode is on.
    #[must_use]
    pub fn prompt_section(self) -> Option<&'static str> {
        match self {
            Self::Ask | Self::Auto => None,
            Self::Plan => Some(
                "# Mode: Plan
You are in PLAN MODE. The user wants a plan they can review before anything is done. You cannot change anything: only tools that read and search the project work, and anything else is refused.

1. Understand the request. If a decision only the user can make would change the whole plan, ask it; otherwise decide sensibly and say so in the plan.
2. Explore the project: its structure, the code the work touches, and its dependency manifests, noting the versions in use.
3. Write the plan as your reply: the goal and context; what you found; the approach and the decisions in it; risks and open questions; concrete steps in order; how the result will be verified.
4. Stop there. The user will comment on it or tell you to go ahead.",
            ),
        }
    }

    /// Whether the tool called `name` is available at all: plan mode offers only the ones that look.
    #[must_use]
    pub fn allows(self, name: &str) -> bool {
        self != Self::Plan || LOOK_ONLY_TOOLS.contains(&name)
    }

    /// Whether the user must say yes before the tool called `name` runs.
    #[must_use]
    pub fn needs_approval(self, name: &str) -> bool {
        self == Self::Ask && !LOOK_ONLY_TOOLS.contains(&name)
    }
}

/// A request from the model to run one tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// Set on assistant messages that ask for tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set on tool messages: which call this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Self::new(Role::Assistant, text)
        }
    }

    #[must_use]
    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::new(Role::Tool, text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_message_round_trips_through_json() {
        let m = Message::new(Role::Assistant, "hi");
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"role":"assistant","text":"hi"}"#);
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), m);
    }

    #[test]
    fn tool_result_carries_the_call_id() {
        let m = Message::tool_result("c1", "ok");
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn thinking_names_its_effort_and_defaults_to_medium() {
        assert_eq!(Thinking::default(), Thinking::Medium);
        let efforts: Vec<_> = Thinking::ALL.iter().map(|t| t.effort()).collect();
        assert_eq!(efforts, [None, Some("low"), Some("medium"), Some("high")]);
    }

    #[test]
    fn plan_withholds_what_changes_things_and_ask_asks_before_it_and_unknown_tools_count_as_changing() {
        for tool in ["write", "edit", "lsp_edit", "repo_clone", "shell", "a_tool_added_next_year"] {
            assert!(!Mode::Plan.allows(tool) && Mode::Auto.allows(tool) && Mode::Ask.allows(tool), "{tool}");
            assert!(Mode::Ask.needs_approval(tool) && !Mode::Auto.needs_approval(tool), "{tool}");
        }
        assert!(Mode::Plan.allows("read") && !Mode::Ask.needs_approval("grep"));
        assert!(Mode::Plan.prompt_section().is_some() && Mode::Ask.prompt_section().is_none());
    }

    #[test]
    fn thread_id_serializes_as_plain_string() {
        let id = ThreadId("t1".into());
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""t1""#);
    }
}
