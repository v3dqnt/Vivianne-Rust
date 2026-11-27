//! Domain types shared by every crate: threads, messages, tool calls, ids.
//! No I/O and no async here; this crate stays dependency-light on purpose.

pub mod attachment;
pub mod context;
pub mod debuglog;

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
const LOOK_ONLY_TOOLS: [&str; 33] = [
    "debug",
    "read",
    "ls",
    "grep",
    "code_search",
    "glob",
    "diagnostics",
    "lsp",
    "repo_overview",
    "recall",
    "reflect",
    "list_threads",
    "read_thread",
    // A subagent is read-only unless the caller is in Auto mode, so starting and collecting them only looks.
    "task",
    "task_output",
    "wait_tasks",
    "list_subagent_profiles",
    // The task board is the agents' own list: changing it changes no file of the project, and a read-only subagent
    // has to be able to claim and complete its task.
    "team_task_create",
    "team_task_list",
    "team_task_get",
    "team_task_update",
    "list_automations",
    // Reads a result that was too large to show; it changes nothing.
    "read_tool_result",
    // Declares files for the user to open; nothing is changed or copied.
    "present",
    // Runs a program that can call only the tools that read.
    "run_code",
    // Moves where relative paths start, inside the project; changes no file.
    "working_directory",
    // Reads the scheduled tasks; creating and editing one are changes and ask.
    "schedule_list",
    "schedule_view",
    "job_output",
    "job_list",
    "terminal_read",
    "terminal_list",
    // Searches this project's saved conversations.
    "search_threads",
];

/// The tools of the `computer` MCP server (agent-desktop), by public name: `mcp__computer__desktop_<tool>`.
pub const COMPUTER_PREFIX: &str = "mcp__computer__desktop_";

/// Of those, the ones that only look at the desktop and need no approval. Anything else of the server (a click, a
/// key press, a launch, the clipboard, a screenshot) changes things or reads what is private, so it asks in Ask mode
/// and is refused in Plan mode, like any tool not on a list: the list names what is safe.
const COMPUTER_LOOK_ONLY: [&str; 12] = [
    "snapshot",
    "find",
    "get",
    "is",
    "wait",
    "list_apps",
    "list_windows",
    "list_displays",
    "list_surfaces",
    "status",
    "permissions",
    "version",
];

/// What the desktop tools return that is private: what an app shows, the windows that are open, the clipboard.
/// Never kept in a saved thread, in memory, or anywhere that outlives the conversation.
const COMPUTER_PRIVATE: [&str; 11] = [
    "snapshot",
    "find",
    "get",
    "screenshot",
    "clipboard_get",
    "list_windows",
    "list_apps",
    "list_surfaces",
    "list_notifications",
    "wait",
    "clipboard_set",
];

/// The tools of the `browser` MCP server (agent-browser), by public name: `mcp__browser__agent_browser_<tool>`.
pub const BROWSER_PREFIX: &str = "mcp__browser__agent_browser_";

/// Of those, the ones that only read the page already open. Opening a page or reading a URL is not here: it sends a
/// request, which is how a page could be made to leak what the agent knows, so it asks.
const BROWSER_LOOK_ONLY: [&str; 11] = [
    "snapshot",
    "get_text",
    "get_value",
    "get_url",
    "get_title",
    "is_visible",
    "wait_ms",
    "wait_for_selector",
    "wait_for_text",
    "wait_for_load",
    "wait_for_url",
];

fn browser_tool(name: &str) -> Option<&str> {
    name.strip_prefix(BROWSER_PREFIX)
}

/// Whether a tool belongs to the browser server.
#[must_use]
pub fn is_browser_tool(name: &str) -> bool {
    browser_tool(name).is_some()
}

/// Whether a tool opens a page or fetches a URL: the calls that send something to a website.
#[must_use]
pub fn browser_navigates(name: &str) -> bool {
    browser_tool(name).is_some_and(|t| t == "open" || t == "read")
}

fn computer_tool(name: &str) -> Option<&str> {
    name.strip_prefix(COMPUTER_PREFIX)
}

/// Whether a tool belongs to the desktop (`computer`) server.
#[must_use]
pub fn is_computer_tool(name: &str) -> bool {
    computer_tool(name).is_some()
}

/// Tools that only read and keep no state between calls, so a run of them in one step may overlap. Everything else (a
/// tool that changes something, one that holds a session such as `debug`, or one this list does not know) runs alone,
/// in the order the model gave it.
const PARALLEL_SAFE_TOOLS: [&str; 12] = [
    "read",
    "ls",
    "grep",
    "code_search",
    "glob",
    "diagnostics",
    "repo_overview",
    "recall",
    "reflect",
    "list_threads",
    "read_thread",
    "read_tool_result",
];

/// Whether a call to this tool may overlap with its neighbours in a step.
#[must_use]
pub fn is_parallel_safe(name: &str) -> bool {
    PARALLEL_SAFE_TOOLS.contains(&name)
}

/// Whether a tool only looks.
#[must_use]
pub fn is_look_only(name: &str) -> bool {
    LOOK_ONLY_TOOLS.contains(&name)
        || computer_tool(name).is_some_and(|t| COMPUTER_LOOK_ONLY.contains(&t))
        || browser_tool(name).is_some_and(|t| BROWSER_LOOK_ONLY.contains(&t))
}

/// Whether what a tool returns is private to the desktop (see [`COMPUTER_PRIVATE`]), or is what a web page said: a
/// page may be the user's mail or bank, so none of the browser's results are kept either.
#[must_use]
pub fn returns_desktop_content(name: &str) -> bool {
    computer_tool(name).is_some_and(|t| COMPUTER_PRIVATE.contains(&t)) || is_browser_tool(name)
}

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
            Self::Plan => {
                "Researches and proposes a plan for review. Changes nothing until you approve it."
            }
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
        self != Self::Plan || is_look_only(name)
    }

    /// Whether the user must say yes before the tool called `name` runs.
    #[must_use]
    pub fn needs_approval(self, name: &str) -> bool {
        self == Self::Ask && !is_look_only(name)
    }
}

/// A request from the model to run one tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// How long one model reply took to arrive, split where a person feels it: the wait before anything came back, and the
/// time the rest took to stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Timing {
    /// From asking to the first thing the model sent, in milliseconds.
    pub first_token_ms: u64,
    /// From that first thing to the end of the reply, in milliseconds.
    pub decode_ms: u64,
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
    /// Images and files the person attached to a user message, as durable references (never paths or bytes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<attachment::AttachmentRef>,
    /// Set when the app, not the person, added this user-role message (instructions, the clock, a referenced session).
    /// A kind this version does not know reads as an ordinary message, so a newer thread still opens.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_context"
    )]
    pub context: Option<context::ContextSource>,
    /// When the message was added, in milliseconds since the Unix epoch. Absent in threads saved before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// Set on assistant messages the agent loop streamed: how long the reply took. Absent in older threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
}

fn lenient_context<'de, D>(deserializer: D) -> Result<Option<context::ContextSource>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()))
}

impl Message {
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            attachments: Vec::new(),
            context: None,
            at: None,
            timing: None,
        }
    }

    /// A user-role message the app adds (not the person), tagged with where it came from.
    #[must_use]
    pub fn context(text: impl Into<String>, source: context::ContextSource) -> Self {
        Self {
            context: Some(source),
            ..Self::new(Role::User, text)
        }
    }

    /// The message with the time it was added.
    #[must_use]
    pub fn at(mut self, millis: u64) -> Self {
        self.at = Some(millis);
        self
    }

    /// The message with how long its reply took to arrive.
    #[must_use]
    pub fn timed(mut self, timing: Timing) -> Self {
        self.timing = Some(timing);
        self
    }

    /// Whether the person typed this: a user message the app did not add.
    #[must_use]
    pub fn is_typed(&self) -> bool {
        self.role == Role::User && self.context.is_none()
    }

    /// The message with these attachments beside its text.
    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<attachment::AttachmentRef>) -> Self {
        self.attachments = attachments;
        self
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
mod parallel_tests {
    use super::*;

    #[test]
    fn only_tools_that_read_and_keep_no_state_may_overlap() {
        for name in ["read", "grep", "glob", "ls", "read_thread"] {
            assert!(is_parallel_safe(name), "{name}");
        }
        for name in ["write", "edit", "shell", "debug", "task", "lsp", "present", "mcp__x__y", "unknown"] {
            assert!(!is_parallel_safe(name), "{name}");
        }
        // Whatever may overlap also only looks: a call that needs approval is never one of them.
        assert!(PARALLEL_SAFE_TOOLS.iter().all(|name| is_look_only(name)));
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
    fn plan_withholds_what_changes_things_and_ask_asks_before_it_and_unknown_tools_count_as_changing()
     {
        for tool in [
            "write",
            "edit",
            "lsp_edit",
            "repo_clone",
            "shell",
            "a_tool_added_next_year",
        ] {
            assert!(
                !Mode::Plan.allows(tool) && Mode::Auto.allows(tool) && Mode::Ask.allows(tool),
                "{tool}"
            );
            assert!(
                Mode::Ask.needs_approval(tool) && !Mode::Auto.needs_approval(tool),
                "{tool}"
            );
        }
        assert!(Mode::Plan.allows("read") && !Mode::Ask.needs_approval("grep"));
        // The desktop: looking is free, doing asks, and Plan refuses everything but looking.
        let (snap, click, shot) = (
            "mcp__computer__desktop_snapshot",
            "mcp__computer__desktop_click",
            "mcp__computer__desktop_screenshot",
        );
        assert!(Mode::Plan.allows(snap) && !Mode::Ask.needs_approval(snap));
        for acting in [
            click,
            shot,
            "mcp__computer__desktop_clipboard_get",
            "mcp__computer__desktop_batch",
            "mcp__computer__desktop_new_next_year",
        ] {
            assert!(
                !Mode::Plan.allows(acting)
                    && Mode::Ask.needs_approval(acting)
                    && !Mode::Auto.needs_approval(acting),
                "{acting}"
            );
        }
        // The browser: reading the open page is free; opening one, clicking and typing ask.
        let page = |t: &str| format!("{BROWSER_PREFIX}{t}");
        assert!(
            Mode::Plan.allows(&page("snapshot")) && !Mode::Ask.needs_approval(&page("get_text"))
        );
        for acting in [
            "open",
            "read",
            "click",
            "fill",
            "eval",
            "cookies_get",
            "a_tool_added_next_year",
        ] {
            let name = page(acting);
            assert!(
                !Mode::Plan.allows(&name) && Mode::Ask.needs_approval(&name),
                "{acting}"
            );
        }
        assert!(browser_navigates(&page("open")) && !browser_navigates(&page("click")));
        assert!(
            returns_desktop_content(&page("get_text"))
                && !is_look_only("mcp__other__agent_browser_snapshot")
        );
        // Another server's tool with a similar name is not the desktop.
        assert!(!is_look_only("mcp__other__desktop_snapshot"));
        assert!(
            returns_desktop_content(snap)
                && returns_desktop_content(shot)
                && !returns_desktop_content(click)
                && !returns_desktop_content("read")
        );
        assert!(Mode::Plan.prompt_section().is_some() && Mode::Ask.prompt_section().is_none());
    }

    #[test]
    fn thread_id_serializes_as_plain_string() {
        let id = ThreadId("t1".into());
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""t1""#);
    }
}
