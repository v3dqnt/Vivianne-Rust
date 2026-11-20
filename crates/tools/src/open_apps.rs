//! "Open in…": the editors installed on this machine, and opening a file or folder in one of them.
//!
//! Detection is a look along PATH for each editor's command line launcher, so only apps the user can actually start
//! from a terminal are offered. Opening runs the launcher with plain arguments (never through a shell), at a line
//! when one is given.

use std::path::Path;

struct Editor {
    id: &'static str,
    label: &'static str,
    /// Command names to look for, first found wins.
    commands: &'static [&'static str],
    /// How a line number is passed.
    goto: Goto,
}

#[derive(Clone, Copy)]
enum Goto {
    /// `--goto file:line`
    Flag,
    /// `file:line`
    Suffix,
    /// The editor takes a plain path only.
    Plain,
}

const EDITORS: &[Editor] = &[
    Editor { id: "vscode", label: "Visual Studio Code", commands: &["code"], goto: Goto::Flag },
    Editor { id: "cursor", label: "Cursor", commands: &["cursor"], goto: Goto::Flag },
    Editor { id: "windsurf", label: "Windsurf", commands: &["windsurf"], goto: Goto::Flag },
    Editor { id: "zed", label: "Zed", commands: &["zed"], goto: Goto::Suffix },
    Editor { id: "sublime", label: "Sublime Text", commands: &["subl"], goto: Goto::Suffix },
    Editor { id: "notepadpp", label: "Notepad++", commands: &["notepad++"], goto: Goto::Plain },
];

/// The app that opens whatever the system opens it with, and the one that shows it in its folder.
pub const DEFAULT: &str = "default";
pub const REVEAL: &str = "reveal";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApp {
    pub id: String,
    pub label: String,
}

/// What to run for an editor: its arguments.
fn plan(editor: &Editor, path: &str, line: Option<u32>) -> Vec<String> {
    match (editor.goto, line) {
        (Goto::Flag, Some(l)) => vec!["--goto".into(), format!("{path}:{l}")],
        (Goto::Suffix, Some(l)) => vec![format!("{path}:{l}")],
        _ => vec![path.to_owned()],
    }
}

fn editor(id: &str) -> Option<&'static Editor> {
    EDITORS.iter().find(|e| e.id == id)
}

fn launcher(e: &Editor) -> Option<std::path::PathBuf> {
    e.commands.iter().find_map(|c| which::which(c).ok())
}

/// The editors found on this machine, plus the system's own ways of opening a file.
#[must_use]
pub fn list() -> Vec<OpenApp> {
    let mut apps: Vec<OpenApp> = EDITORS.iter().filter(|e| launcher(e).is_some()).map(|e| OpenApp { id: e.id.into(), label: e.label.into() }).collect();
    apps.push(OpenApp { id: DEFAULT.into(), label: "Default app".into() });
    apps.push(OpenApp { id: REVEAL.into(), label: "Show in folder".into() });
    apps
}

/// Opens `path` in the app named by `app` (an id from [`list`]), at `line` when the app can.
pub fn open_in(path: &str, app: &str, line: Option<u32>) -> Result<(), String> {
    let p = Path::new(path);
    if !p.is_absolute() || !p.exists() {
        return Err(format!("{path} isn't a file or folder on this machine."));
    }
    match app {
        DEFAULT => open_default(p),
        REVEAL => reveal(p),
        id => {
            let e = editor(id).ok_or_else(|| format!("Unknown app '{id}'."))?;
            let exe = launcher(e).ok_or_else(|| format!("{} isn't installed (its command line launcher wasn't found).", e.label))?;
            let mut cmd = crate::proc::quiet_std(exe);
            cmd.args(plan(e, path, line));
            cmd.spawn().map(|_| ()).map_err(|er| format!("Couldn't start {}: {er}", e.label))
        }
    }
}

fn open_default(p: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let spawned = crate::proc::quiet_std("cmd").args(["/C", "start", ""]).arg(p).spawn();
    #[cfg(target_os = "macos")]
    let spawned = crate::proc::quiet_std("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = crate::proc::quiet_std("xdg-open").arg(p).spawn();
    spawned.map(|_| ()).map_err(|e| format!("couldn't open {}: {e}", p.display()))
}

fn reveal(p: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let spawned = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(if p.is_dir() { p } else { p.parent().unwrap_or(p) }).spawn();
    spawned.map(|_| ()).map_err(|e| format!("couldn't show {} in its folder: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_named(id: &str) -> &'static Editor {
        editor(id).expect("a known editor")
    }

    #[test]
    fn a_line_is_passed_the_way_each_editor_wants_it() {
        assert_eq!(plan(editor_named("vscode"), "/w/a.rs", Some(12)), vec!["--goto", "/w/a.rs:12"]);
        assert_eq!(plan(editor_named("zed"), "/w/a.rs", Some(12)), vec!["/w/a.rs:12"]);
        assert_eq!(plan(editor_named("notepadpp"), "/w/a.rs", Some(12)), vec!["/w/a.rs"]);
        assert_eq!(plan(editor_named("vscode"), "/w/a.rs", None), vec!["/w/a.rs"]);
    }

    #[test]
    fn ids_are_unique_unknown_apps_and_relative_paths_are_refused_and_the_system_choices_are_always_offered() {
        let mut ids: Vec<&str> = EDITORS.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), EDITORS.len());
        assert!(open_in("relative/path", "vscode", None).is_err());
        let here = std::env::current_dir().expect("a current folder").display().to_string();
        assert!(open_in(&here, "nope", None).expect_err("not an app").contains("Unknown app"));
        let apps = list();
        assert!(apps.iter().any(|a| a.id == DEFAULT) && apps.iter().any(|a| a.id == REVEAL));
    }
}
