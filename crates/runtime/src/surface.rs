//! What every surface that runs the agent (the app, `viv`, the remote server) adds to its toolbox, written once (R1.4):
//! the conversation tools, the schedule tools, and the standing sandbox mode from the settings.

use std::path::Path;

use vivianne_tools::Toolbox;
use vivianne_tools::confinement::mode_from_setting;

use crate::config::Resolved;
use crate::schedule::{store::Store, tools};
use crate::thread_tools::{ListThreads, ReadThread, SearchThreads};

/// Adds the ported session and schedule tools to `toolbox` and sets its file mode. Returns what to warn the person of.
/// Scheduled tasks are delivered by the app while it is open; the other surfaces can still manage them.
#[must_use]
pub fn prepare(toolbox: &mut Toolbox, workspace: &Path, settings: &Resolved) -> Option<String> {
    let folder = workspace.display().to_string();
    toolbox.add(ListThreads::new(&folder));
    toolbox.add(ReadThread::new(&folder));
    toolbox.add(SearchThreads::new(&folder));
    let schedules = Store::standard();
    toolbox.add(tools::ScheduleCreate(schedules.clone()));
    toolbox.add(tools::ScheduleList(schedules.clone()));
    toolbox.add(tools::ScheduleView(schedules.clone()));
    toolbox.add(tools::ScheduleEdit(schedules));
    let (mode, warning) = mode_from_setting(settings.sandbox.value.as_deref());
    toolbox.confinement().set(mode);
    toolbox.jobs().keep_output();
    warning
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_gets_the_same_tools_and_the_configured_mode() {
        let dir = std::env::temp_dir();
        let mut settings = crate::config::resolve_with(
            &dir,
            None,
            &|_| None,
            &serde_json::json!({}),
            &crate::config::ProjectConfig::default(),
        );
        let mut toolbox = Toolbox::coding(&dir);
        assert_eq!(prepare(&mut toolbox, &dir, &settings), None);
        let names: Vec<String> = toolbox.definitions().into_iter().map(|d| d.name).collect();
        for name in [
            "list_threads",
            "read_thread",
            "search_threads",
            "schedule_create",
            "schedule_list",
            "schedule_view",
            "schedule_edit",
        ] {
            assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{name}");
        }
        assert!(
            !toolbox.confinement().is_confined(),
            "nothing is confined unless asked"
        );
        settings.sandbox.value = Some("read-only".to_owned());
        let mut confined = Toolbox::coding(&dir);
        let _ = prepare(&mut confined, &dir, &settings);
        assert!(confined.confinement().is_confined());
        let shell = confined
            .definitions()
            .into_iter()
            .find(|d| d.name == "shell")
            .expect("shell");
        assert!(shell.parameters["properties"]["sandbox_permissions"].is_object());
    }
}
