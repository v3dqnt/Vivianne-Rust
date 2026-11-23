//! Reading what a coding agent in a terminal pane is doing from its screen: working, blocked on the person, or
//! idle. Which agent runs in the pane comes from its process; what it is doing comes from rules in TOML
//! manifests, so a new agent or a changed screen is a file edit, not a release.
//!
//! The engine follows herdr's design (rules with a priority, a region of the screen they look at, and limits on
//! how big a manifest may be). The bundled rules are **new work, not herdr's**: they are starting points written
//! from how these agents usually look and need tuning against real output. A person's own rules in
//! `~/.viv/agent-detection/<agent>.toml` replace the bundled ones for that agent.

use regex::Regex;
use serde::Deserialize;

use crate::protocol::AgentState;

pub const MAX_RULES: usize = 128;
pub const MAX_GATE_DEPTH: usize = 8;
pub const MAX_GATES: usize = 512;
pub const MAX_MATCHERS: usize = 1024;
pub const MAX_MATCHER_CHARS: usize = 512;

/// The part of the screen a rule looks at.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// All of the recent screen text.
    WholeRecent,
    /// Everything after the last prompt marker (`>`, `❯`, `$`).
    AfterLastPromptMarker,
    BottomLines(usize),
    OscTitle,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Gate {
    #[serde(default)]
    pub contains: Vec<String>,
    #[serde(default)]
    pub regex: Vec<String>,
    #[serde(default)]
    pub all: Vec<Gate>,
    #[serde(default)]
    pub any: Vec<Gate>,
    #[serde(default)]
    pub not: Vec<Gate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    pub state: String,
    #[serde(default)]
    pub priority: i32,
    pub region: Region,
    #[serde(flatten)]
    pub gate: Gate,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub rules: Vec<Rule>,
}

/// What a manifest decided, and why (for a person debugging a rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub state: AgentState,
    pub rule: Option<String>,
}

fn parse_state(text: &str) -> Result<AgentState, String> {
    Ok(match text {
        "idle" => AgentState::Idle,
        "working" => AgentState::Working,
        "blocked" => AgentState::Blocked,
        "unknown" => AgentState::Unknown,
        other => {
            return Err(format!(
                "`{other}` is not a state (idle, working, blocked, unknown)"
            ));
        }
    })
}

fn count(gate: &Gate, depth: usize, gates: &mut usize, matchers: &mut usize) -> Result<(), String> {
    if depth > MAX_GATE_DEPTH {
        return Err(format!("a rule nests deeper than {MAX_GATE_DEPTH}"));
    }
    *gates += 1;
    *matchers += gate.contains.len() + gate.regex.len();
    if gate
        .contains
        .iter()
        .chain(&gate.regex)
        .any(|m| m.chars().count() > MAX_MATCHER_CHARS)
    {
        return Err(format!(
            "a matcher is longer than {MAX_MATCHER_CHARS} characters"
        ));
    }
    for sub in gate.all.iter().chain(&gate.any).chain(&gate.not) {
        count(sub, depth + 1, gates, matchers)?;
    }
    if *gates > MAX_GATES || *matchers > MAX_MATCHERS {
        return Err("the manifest has too many conditions".to_owned());
    }
    Ok(())
}

impl Manifest {
    /// Reads a manifest and checks it against the limits.
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if manifest.rules.len() > MAX_RULES {
            return Err(format!("a manifest has at most {MAX_RULES} rules"));
        }
        let (mut gates, mut matchers) = (0, 0);
        for rule in &manifest.rules {
            parse_state(&rule.state)?;
            count(&rule.gate, 0, &mut gates, &mut matchers)?;
            check_regexes(&rule.gate)?;
        }
        Ok(manifest)
    }

    /// Evaluates every rule; the highest priority that matches wins (the first listed, on a tie). With none, an agent
    /// that is known is idle.
    #[must_use]
    pub fn detect(&self, screen: &Screen) -> Detection {
        let mut best: Option<(&Rule, AgentState)> = None;
        for rule in &self.rules {
            let Ok(state) = parse_state(&rule.state) else {
                continue;
            };
            if matches(&rule.gate, &region_text(&rule.region, screen))
                && best.is_none_or(|(b, _)| rule.priority > b.priority)
            {
                best = Some((rule, state));
            }
        }
        best.map_or(
            Detection {
                state: AgentState::Idle,
                rule: None,
            },
            |(r, s)| Detection {
                state: s,
                rule: Some(r.id.clone()),
            },
        )
    }
}

fn check_regexes(gate: &Gate) -> Result<(), String> {
    for r in &gate.regex {
        Regex::new(r).map_err(|e| format!("a regex is not valid: {e}"))?;
    }
    for sub in gate.all.iter().chain(&gate.any).chain(&gate.not) {
        check_regexes(sub)?;
    }
    Ok(())
}

/// A terminal's text, as detection reads it.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    /// The recent lines, oldest first.
    pub lines: Vec<String>,
    pub osc_title: String,
}

fn region_text(region: &Region, screen: &Screen) -> String {
    match region {
        Region::WholeRecent => screen.lines.join("\n"),
        Region::OscTitle => screen.osc_title.clone(),
        Region::BottomLines(n) => {
            let start = screen.lines.len().saturating_sub(*n);
            screen.lines[start..].join("\n")
        }
        Region::AfterLastPromptMarker => {
            let at = screen.lines.iter().rposition(|l| {
                let t = l.trim_start();
                t.starts_with('>') || t.starts_with('❯') || t.starts_with('$')
            });
            screen.lines[at.map_or(0, |i| i + 1)..].join("\n")
        }
    }
}

fn matches(gate: &Gate, text: &str) -> bool {
    let own = gate.contains.iter().all(|c| text.contains(c.as_str()))
        && gate
            .regex
            .iter()
            .all(|r| Regex::new(r).is_ok_and(|re| re.is_match(text)));
    own && gate.all.iter().all(|g| matches(g, text))
        && (gate.any.is_empty() || gate.any.iter().any(|g| matches(g, text)))
        && !gate.not.iter().any(|g| matches(g, text))
}

/// The agent a process is, from its command line: the program's name, or the package a wrapper (`node`, `bun`,
/// `python`) runs. `None` for anything else.
#[must_use]
pub fn identify_agent(command_line: &[String]) -> Option<&'static str> {
    const KNOWN: [(&str, &str); 6] = [
        ("claude", "claude"),
        ("codex", "codex"),
        ("gemini", "gemini"),
        ("opencode", "opencode"),
        ("cursor-agent", "cursor"),
        ("viv", "vivianne"),
    ];
    let words: Vec<String> = command_line
        .iter()
        .take(4)
        .map(|w| w.replace('\\', "/").to_ascii_lowercase())
        .collect();
    for word in &words {
        let name = word.rsplit('/').next().unwrap_or(word);
        let name = name.strip_suffix(".exe").unwrap_or(name);
        let name = name.strip_suffix(".js").unwrap_or(name);
        if let Some((_, id)) = KNOWN.iter().find(|(n, _)| *n == name) {
            return Some(id);
        }
        // A package path such as node_modules/@anthropic-ai/claude-code/cli.js.
        if let Some((_, id)) = KNOWN
            .iter()
            .find(|(n, _)| word.contains(&format!("/{n}-")) || word.contains(&format!("/{n}/")))
        {
            return Some(id);
        }
    }
    None
}

/// The manifests that ship with the app. Starting points: see the module's note.
pub const BUNDLED: [(&str, &str); 3] = [
    (
        "claude",
        r#"
id = "claude"
version = 1
[[rules]]
id = "asks-permission"
state = "blocked"
priority = 30
region = "whole_recent"
any = [{ contains = ["Do you want to"] }, { contains = ["Allow this"] }, { regex = ["\\(y/n\\)"] }]
[[rules]]
id = "working"
state = "working"
priority = 20
region = { bottom_lines = 8 }
regex = ["esc to interrupt"]
"#,
    ),
    (
        "codex",
        r#"
id = "codex"
version = 1
[[rules]]
id = "asks-approval"
state = "blocked"
priority = 30
region = "whole_recent"
any = [{ contains = ["Would you like to run"] }, { contains = ["Allow command"] }]
[[rules]]
id = "working"
state = "working"
priority = 20
region = "whole_recent"
regex = ["Working \\(\\d+"]
"#,
    ),
    (
        "vivianne",
        r#"
id = "vivianne"
version = 1
[[rules]]
id = "asks"
state = "blocked"
priority = 30
region = "whole_recent"
contains = ["[asks]"]
"#,
    ),
];

/// The manifest for an agent: the person's own file, else the bundled one.
#[must_use]
pub fn manifest_for(agent: &str) -> Option<Manifest> {
    let own = vivianne_ai::paths::viv_home_path(&["agent-detection", &format!("{agent}.toml")]);
    if let Some(m) = std::fs::read_to_string(own)
        .ok()
        .and_then(|t| Manifest::parse(&t).ok())
    {
        return Some(m);
    }
    BUNDLED
        .iter()
        .find(|(id, _)| *id == agent)
        .and_then(|(_, text)| Manifest::parse(text).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(lines: &[&str]) -> Screen {
        Screen {
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
            osc_title: String::new(),
        }
    }

    #[test]
    fn every_bundled_manifest_parses_within_the_limits() {
        for (id, text) in BUNDLED {
            let m = Manifest::parse(text).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(m.id, id);
        }
    }

    #[test]
    fn the_highest_priority_match_wins_and_a_known_agent_with_no_match_is_idle() {
        let m = manifest_for("claude").expect("bundled");
        let working = m.detect(&screen(&["thinking…", "esc to interrupt"]));
        assert_eq!(
            (working.state, working.rule.as_deref()),
            (AgentState::Working, Some("working"))
        );
        // Both match; blocked has the higher priority.
        let both = m.detect(&screen(&["Do you want to proceed?", "esc to interrupt"]));
        assert_eq!(both.state, AgentState::Blocked);
        assert_eq!(m.detect(&screen(&["> "])).state, AgentState::Idle);
    }

    #[test]
    fn regions_limit_where_a_rule_looks() {
        let m = Manifest::parse(
            r#"
id = "x"
[[rules]]
id = "bottom"
state = "working"
region = "bottom_lines"
contains = ["busy"]
"#,
        );
        // `bottom_lines` is a tuple variant in TOML as { bottom_lines = N }; a bare string is refused.
        assert!(m.is_err());
        let m = Manifest::parse(
            r#"
id = "x"
[[rules]]
id = "bottom"
state = "working"
region = { bottom_lines = 2 }
contains = ["busy"]
"#,
        )
        .expect("parses");
        assert_eq!(
            m.detect(&screen(&["busy", "a", "b"])).state,
            AgentState::Idle,
            "too far up"
        );
        assert_eq!(
            m.detect(&screen(&["a", "b", "busy"])).state,
            AgentState::Working
        );
    }

    #[test]
    fn a_bad_manifest_is_refused_not_guessed_at() {
        assert!(
            Manifest::parse(
                "id = \"x\"\n[[rules]]\nid=\"r\"\nstate=\"asleep\"\nregion=\"whole_recent\""
            )
            .is_err()
        );
        assert!(Manifest::parse("id = \"x\"\n[[rules]]\nid=\"r\"\nstate=\"idle\"\nregion=\"whole_recent\"\nregex=[\"(\"]").is_err());
        let long = "a".repeat(MAX_MATCHER_CHARS + 1);
        assert!(Manifest::parse(&format!("id=\"x\"\n[[rules]]\nid=\"r\"\nstate=\"idle\"\nregion=\"whole_recent\"\ncontains=[\"{long}\"]")).is_err());
    }

    #[test]
    fn the_agent_is_named_from_its_command_line_through_wrappers() {
        let cmd = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(identify_agent(&cmd("claude --resume")), Some("claude"));
        assert_eq!(
            identify_agent(&cmd("C:\\Users\\me\\bin\\codex.exe")),
            Some("codex")
        );
        assert_eq!(
            identify_agent(&cmd(
                "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            )),
            Some("claude")
        );
        assert_eq!(identify_agent(&cmd("vim notes.txt")), None);
    }
}
