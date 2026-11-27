//! What every surface that runs the agent (the app, `viv`, the remote server) adds to its toolbox, written once (R1.4):
//! the conversation tools, the schedule tools, and the standing sandbox mode from the settings.

use std::path::Path;
use std::sync::Arc;

use vivianne_tools::Toolbox;
use vivianne_tools::confinement::mode_from_setting;

use crate::config::Resolved;
use crate::orchestration;
use crate::schedule::{store::Store, tools};
use crate::thread_tools::{ListThreads, ReadThread, SearchThreads};

/// Adds the ported session and schedule tools to `toolbox` and sets its file mode. Returns what to warn the person of.
/// Scheduled tasks are delivered by the app while it is open; the other surfaces can still manage them.
#[must_use]
pub fn prepare(toolbox: &mut Toolbox, workspace: &Path, settings: &Resolved) -> Option<String> {
    orchestration::set_spawner(Arc::new(orchestration::StandardSpawner));
    toolbox.add(orchestration::Task);
    toolbox.add(orchestration::TaskOutput);
    toolbox.add(orchestration::WaitTasks);
    toolbox.add(orchestration::KillTask);
    toolbox.add(orchestration::ListSubagentProfiles);
    prepare_child(toolbox, workspace, settings)
}

/// [`prepare`] for a subagent's toolbox: everything but the tools that start and collect subagents.
#[must_use]
pub fn prepare_child(
    toolbox: &mut Toolbox,
    workspace: &Path,
    settings: &Resolved,
) -> Option<String> {
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
    // Only when asked for: four more tools in every request.
    if settings.team.value {
        let boards = orchestration::team::Store::standard();
        for kind in orchestration::team::Kind::ALL {
            toolbox.add(orchestration::team::TeamTool(kind, boards.clone()));
        }
    }
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
            "task",
            "task_output",
            "wait_tasks",
            "kill_task",
            "list_subagent_profiles",
        ] {
            assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{name}");
        }
        assert!(
            !toolbox.confinement().is_confined(),
            "nothing is confined unless asked"
        );
        let has_board = |t: &Toolbox| t.get("team_task_update").is_some();
        assert!(!has_board(&toolbox), "off unless asked for");
        settings.team.value = true;
        let mut team = Toolbox::coding(&dir);
        let _ = prepare(&mut team, &dir, &settings);
        assert!(has_board(&team));
        let mut child = Toolbox::coding(&dir);
        let _ = prepare_child(&mut child, &dir, &settings);
        assert!(has_board(&child) && child.get("task").is_none());
        settings.team.value = false;
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
