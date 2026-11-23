//! Where the agent sits in tmux. On the first step of a turn, when this process really runs in a tmux pane and the
//! pane's layout changed since the last reading, the model gets one message naming the session, window and pane. Port of
//! `tmux-context`.
//!
//! A terminal that only inherited `$TMUX_PANE` from an ancestor (an editor's integrated terminal) is not in that pane, so
//! the pane's tty must match this process's own. Any failure, and any machine with no tmux, adds nothing and never fails a turn.

use vivianne_core::Message;
use vivianne_core::context::ContextSource;

/// The fields asked of tmux, in order.
const FIELDS: [&str; 8] = [
    "#{session_name}",
    "#{window_index}",
    "#{window_name}",
    "#{pane_index}",
    "#{pane_id}",
    "#{window_active}",
    "#{pane_active}",
    "#{window_layout}",
];
/// tmux does not read C escapes in a format, so this two-character text is emitted as it is and split on here.
const SEPARATOR: &str = "\\t";
const PREFIX: &str = "tmux location (turn ";

/// Where one pane is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub session_name: String,
    pub window_index: String,
    pub window_name: String,
    pub pane_index: String,
    pub pane_id: String,
    pub window_active: String,
    pub pane_active: String,
    pub window_layout: String,
}

/// Runs a command on the machine and says what it printed, or `None` for a nonzero exit or a failure to run.
pub trait Runner {
    fn run(&self, program: &str, args: &[&str]) -> Option<String>;
}

/// The command string the original sends: a no-op unless `$TMUX_PANE` is set and its tty is this process's own.
#[must_use]
pub fn query_script(process_id: u32) -> String {
    let format = FIELDS.join(SEPARATOR);
    [
        "[ -n \"$TMUX_PANE\" ] || exit 1".to_owned(),
        format!("self_tty=$(ps -o tty= -p {process_id} | tr -d ' ')"),
        "[ -n \"$self_tty\" ] || exit 1".to_owned(),
        "pane_tty=$(tmux display-message -t \"$TMUX_PANE\" -p '#{pane_tty}') || exit 1".to_owned(),
        "[ \"$pane_tty\" = \"/dev/$self_tty\" ] || exit 1".to_owned(),
        format!("exec tmux display-message -t \"$TMUX_PANE\" -p '{format}'"),
    ]
    .join("\n")
}

/// Reads one line of tmux output into a location. `None` unless all eight fields are there and the pane has an id.
#[must_use]
pub fn parse_location(output: &str) -> Option<Location> {
    let line = output.lines().next()?;
    let parts: Vec<&str> = line.split(SEPARATOR).collect();
    if parts.len() != FIELDS.len() || parts[4].is_empty() {
        return None;
    }
    Some(Location {
        session_name: parts[0].to_owned(),
        window_index: parts[1].to_owned(),
        window_name: parts[2].to_owned(),
        pane_index: parts[3].to_owned(),
        pane_id: parts[4].to_owned(),
        window_active: parts[5].to_owned(),
        pane_active: parts[6].to_owned(),
        window_layout: parts[7].to_owned(),
    })
}

/// The part of a reading compared to decide whether anything changed: it leaves out the turn, so a move re-injects and a
/// new turn alone does not.
#[must_use]
pub fn render_state(location: &Location) -> String {
    format!(
        "session {}, window {} {}, pane {} {}\nwindow active={}, pane active={}, layout {}",
        location.session_name,
        location.window_index,
        serde_json::to_string(&location.window_name).unwrap_or_default(),
        location.pane_index,
        location.pane_id,
        location.window_active,
        location.pane_active,
        location.window_layout
    )
}

/// The full text of a reading.
#[must_use]
pub fn render_reading(location: &Location, turn: usize) -> String {
    format!("{PREFIX}{turn}):\n{}", render_state(location))
}

/// The state block of the newest reading in the conversation, and when it was added.
fn previous_reading(history: &[Message]) -> Option<(String, Option<u64>)> {
    let m = history
        .iter()
        .rev()
        .find(|m| matches!(m.context, Some(ContextSource::TmuxContext)))?;
    let state = m
        .text
        .split_once('\n')
        .map_or("", |(_, rest)| rest)
        .to_owned();
    Some((state, m.at))
}

/// Asks tmux where this process is, through `runner`. `None` when not in a pane, or on any failure.
pub fn query(runner: &dyn Runner, process_id: u32) -> Option<Location> {
    let script = query_script(process_id);
    parse_location(&runner.run("sh", &["-c", &script])?)
}

/// The reading to put before the person's message on the first step of a turn, if the location changed.
#[must_use]
pub fn reading(
    history: &[Message],
    location: &Location,
    turn: usize,
    now_ms: u64,
    refresh_interval_ms: u64,
) -> Option<Message> {
    let state = render_state(location);
    if let Some((previous, at)) = previous_reading(history) {
        if refresh_interval_ms > 0
            && at.is_some_and(|t| now_ms >= t && now_ms - t < refresh_interval_ms)
        {
            return None;
        }
        if previous == state {
            return None;
        }
    }
    Some(Message::context(render_reading(location, turn), ContextSource::TmuxContext).at(now_ms))
}
