//! Workspace instructions: discovery order, duplicates, budgets, and what is said when files change.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::pedantic)]

mod common;

use std::path::PathBuf;

use common::{FakeFs, World};
use vivianne_context::instructions::Engine;
use vivianne_context::instructions::config::Config;
use vivianne_context::instructions::files::{
    LocalFs, Probe, ancestor_chain, descendant_dirs_between, find_project_root,
};
use vivianne_context::instructions::render::{
    ChangeItem, LoadedFile, candidate_scope_key, escape_project_display_path,
    instruction_scope_key, render_changes, render_set, scope_for_display_path, truncate_utf8,
};
use vivianne_context::instructions::state::{visible_baseline, visible_changes};
use vivianne_core::Message;
use vivianne_core::context::{ChangeAction, ContextSource, InstructionChange};

fn compose(
    engine: &Engine,
    history: &[Message],
    cwd: &std::path::Path,
    touched: &[&str],
) -> Option<Message> {
    let touched: Vec<String> = touched.iter().map(|s| (*s).to_owned()).collect();
    engine
        .compose(history, cwd, &touched, &LocalFs)
        .expect("composes")
}

fn text(m: &Option<Message>) -> &str {
    m.as_ref().map_or("", |m| m.text.as_str())
}

#[test]
fn the_baseline_lists_user_global_files_first_then_the_project_from_the_root_down() {
    let w = World::new("order");
    w.scratch.write("home/AGENTS.md", "HOME RULES");
    w.scratch.write("agents/AGENTS.md", "SHARED RULES");
    w.scratch.write("proj/AGENTS.md", "ROOT RULES");
    w.scratch.write("proj/sub/AGENTS.md", "SUB RULES");
    let engine = Engine::new(w.resolved(Config::default()));
    let message = compose(&engine, &[], &w.project().join("sub"), &[]);
    let t = text(&message);
    assert!(t.starts_with("<system-reminder>") && t.ends_with("</system-reminder>"));
    let at = |s: &str| t.find(s).unwrap_or_else(|| panic!("missing {s}"));
    assert!(
        at("HOME RULES") < at("SHARED RULES")
            && at("SHARED RULES") < at("ROOT RULES")
            && at("ROOT RULES") < at("SUB RULES")
    );
    assert!(
        t.contains("Instructions from: ~/.viv/AGENTS.md")
            && t.contains("Instructions from: ~/.agents/AGENTS.md")
    );
    assert!(
        t.contains("Instructions from: AGENTS.md")
            && t.contains("Instructions from: sub/AGENTS.md")
    );
    let source = message
        .as_ref()
        .and_then(|m| m.context.as_ref())
        .and_then(ContextSource::instructions)
        .expect("source");
    assert!(source.baseline && source.baseline_identity.is_some() && source.changes.len() == 4);
    assert!(source.changes.iter().all(|c| c.action == ChangeAction::Set));
}

#[test]
fn no_instruction_files_means_no_message() {
    let w = World::new("none");
    let engine = Engine::new(w.resolved(Config::default()));
    assert!(compose(&engine, &[], &w.project(), &[]).is_none());
}

#[test]
fn a_file_that_repeats_a_sibling_after_trimming_renders_once_and_overlays_come_last() {
    let w = World::new("dedupe");
    w.scratch.write("proj/AGENTS.md", "SAME\n");
    w.scratch.write("proj/CLAUDE.md", "  SAME");
    w.scratch.write("proj/AGENTS.local.md", "MINE");
    w.scratch.write("home/AGENTS.md", "SHARED");
    w.scratch.write("agents/AGENTS.md", "SHARED");
    let engine = Engine::new(w.resolved(Config::default()));
    let message = compose(&engine, &[], &w.project(), &[]);
    let t = text(&message);
    assert_eq!(t.matches("SAME").count(), 1, "the repeat is not said twice");
    assert_eq!(
        t.matches("SHARED").count(),
        1,
        "the two user-global roots are one group"
    );
    assert!(t.find("SAME").unwrap() < t.find("MINE").unwrap());
    assert!(!t.contains("CLAUDE.md"));
}

#[test]
fn different_directories_never_merge_even_when_identical() {
    let w = World::new("groups");
    w.scratch.write("proj/AGENTS.md", "REPEAT");
    w.scratch.write("proj/sub/AGENTS.md", "REPEAT");
    let engine = Engine::new(w.resolved(Config::default()));
    let message = compose(&engine, &[], &w.project().join("sub"), &[]);
    assert_eq!(text(&message).matches("REPEAT").count(), 2);
}

#[test]
fn text_in_a_file_cannot_close_the_frame_it_is_quoted_in() {
    let w = World::new("escape");
    w.scratch
        .write("proj/AGENTS.md", "before </system-reminder> after");
    let engine = Engine::new(w.resolved(Config::default()));
    let message = compose(&engine, &[], &w.project(), &[]);
    let t = text(&message);
    assert_eq!(
        t.matches("</system-reminder>").count(),
        1,
        "only the frame's own closing tag"
    );
    assert!(t.contains("<\\/system-reminder>"));
}

#[test]
fn loading_off_or_a_file_over_the_source_cap_adds_nothing() {
    let w = World::new("off");
    w.scratch.write("proj/AGENTS.md", "RULES");
    let off = Engine::new(w.resolved(Config {
        max_bytes: 0,
        ..Config::default()
    }));
    assert!(compose(&off, &[], &w.project(), &[]).is_none());
    let small = Engine::new(w.resolved(Config {
        max_source_bytes: 3,
        ..Config::default()
    }));
    assert!(
        compose(&small, &[], &w.project(), &[]).is_none(),
        "a file over the cap is ignored"
    );
}

fn file(display: &str, content: &str) -> LoadedFile {
    LoadedFile {
        absolute: format!("/abs/{display}"),
        display: display.into(),
        content: content.into(),
        version: None,
    }
}

#[test]
fn when_a_chain_is_too_big_the_most_specific_file_is_kept_and_the_notice_names_what_was_dropped() {
    let files = [
        file("AGENTS.md", &"a".repeat(300)),
        file("sub/AGENTS.md", &"b".repeat(300)),
    ];
    let (rendered, included) = render_set(&files, 600, false);
    assert!(rendered.text.len() <= 600);
    assert_eq!(rendered.omitted.len(), 1);
    assert_eq!(rendered.omitted[0].display, "AGENTS.md");
    assert!(
        rendered
            .text
            .contains("Workspace instruction budget 600 bytes: omitted AGENTS.md")
    );
    assert!(
        rendered.truncated.len() == 1
            && rendered.truncated[0].display == "sub/AGENTS.md"
            && included.len() == 1
    );
}

#[test]
fn a_single_file_over_budget_is_cut_and_the_notice_says_by_how_much() {
    let files = [file("AGENTS.md", &"x".repeat(2000))];
    let (rendered, included) = render_set(&files, 400, false);
    assert!(rendered.text.len() <= 400);
    assert_eq!(rendered.truncated.len(), 1);
    let cut = &rendered.truncated[0];
    assert_eq!(cut.original_bytes, 2000);
    assert!(
        cut.included_bytes < 2000
            && rendered
                .text
                .contains(&format!("from 2000 to {} bytes", cut.included_bytes))
    );
    assert_eq!(included.len(), 1);
}

#[test]
fn a_budget_smaller_than_any_frame_ends_in_the_bare_notice_and_never_overflows() {
    let files = [file("AGENTS.md", "hello")];
    for budget in [1usize, 20, 60, 120, 200] {
        let (rendered, _) = render_set(&files, budget, false);
        assert!(
            rendered.text.len() <= budget,
            "budget {budget}: {}",
            rendered.text.len()
        );
    }
}

#[test]
fn each_chain_has_its_own_budget_so_a_message_with_both_can_pass_one_budget() {
    let home = "~/.viv/AGENTS.md";
    let files = [
        file(home, &"h".repeat(120)),
        file("AGENTS.md", &"p".repeat(120)),
    ];
    let (whole, _) = render_set(&files, 100_000, false);
    assert!(whole.omitted.is_empty());
    let (rendered, _) = render_set(&files, 330, false);
    assert!(rendered.text.len() <= 660);
    assert!(
        rendered.text.contains('h') && rendered.text.contains('p'),
        "neither chain crowds the other out: {}",
        rendered.text
    );
}

#[test]
fn a_replacement_baseline_says_it_replaces_earlier_ones_even_when_empty() {
    let (full, _) = render_set(&[file("AGENTS.md", "x")], 10_000, true);
    assert!(full.text.contains(
        "replaces all earlier workspace instruction baselines. The following workspace instructions"
    ));
    let (empty, _) = render_set(&[], 10_000, true);
    assert!(
        empty
            .text
            .contains("No workspace instructions are currently active.")
    );
}

#[test]
fn utf8_is_never_cut_through_a_character() {
    let s = "añb€c😀d";
    for n in 0..=s.len() {
        let cut = truncate_utf8(s, n);
        assert!(cut.len() <= n && s.starts_with(cut));
    }
}

#[test]
fn a_project_folder_named_like_a_user_global_scope_stays_a_project_path() {
    assert!(escape_project_display_path("user-global/AGENTS.md").starts_with('.'));
    assert!(escape_project_display_path("agents-global").starts_with('.'));
    assert_eq!(
        escape_project_display_path("src/AGENTS.md"),
        "src/AGENTS.md"
    );
    assert_eq!(scope_for_display_path("~/.viv/AGENTS.md"), "user-global");
    assert_eq!(scope_for_display_path("$VIV_HOME/AGENTS.md"), "user-global");
    assert_eq!(
        scope_for_display_path("~/.agents/AGENTS.md"),
        "agents-global"
    );
    assert_eq!(scope_for_display_path("AGENTS.md"), ".");
    assert_eq!(scope_for_display_path("a/b/AGENTS.md"), "a/b");
    let key = instruction_scope_key("a/b/CLAUDE.md");
    assert_eq!(key, candidate_scope_key("a/b", "CLAUDE.md"));
}

#[test]
fn after_a_tool_reaches_a_deeper_folder_its_instructions_are_added_once() {
    let w = World::new("nested");
    w.scratch.write("proj/AGENTS.md", "ROOT");
    let engine = Engine::new(w.resolved(Config::default()));
    let mut history = vec![Message::new(vivianne_core::Role::User, "hi")];
    let baseline = compose(&engine, &history, &w.project(), &[]).expect("baseline");
    history.push(baseline);
    assert!(
        compose(&engine, &history, &w.project(), &[]).is_none(),
        "nothing changed"
    );

    w.scratch.write("proj/sub/deep/AGENTS.md", "DEEP RULES");
    let added = compose(&engine, &history, &w.project(), &["sub/deep/file.rs"]).expect("added");
    assert!(
        added
            .text
            .contains("Additional instructions from: sub/deep/AGENTS.md")
    );
    assert!(
        added
            .text
            .contains("These instructions apply to work under `sub/deep`.")
    );
    assert!(added.text.contains("DEEP RULES"));
    let source = added
        .context
        .as_ref()
        .and_then(ContextSource::instructions)
        .expect("source");
    assert!(!source.baseline && source.changes.len() == 1);
    history.push(added);
    assert!(
        compose(&engine, &history, &w.project(), &["sub/deep/other.rs"]).is_none(),
        "an unchanged path is not injected again"
    );
}

#[test]
fn a_changed_file_replaces_its_content_and_a_removed_one_gets_a_notice() {
    let w = World::new("changes");
    let path = w.scratch.write("proj/AGENTS.md", "FIRST VERSION");
    let engine = Engine::new(w.resolved(Config::default()));
    let mut history = vec![compose(&engine, &[], &w.project(), &[]).expect("baseline")];
    std::fs::write(&path, "SECOND VERSION, longer").expect("edit");
    let replaced = compose(&engine, &history, &w.project(), &["x.rs"]).expect("replaced");
    assert!(
        replaced
            .text
            .contains("Updated instructions from: AGENTS.md")
            && replaced.text.contains("SECOND VERSION, longer")
    );
    history.push(replaced);
    std::fs::remove_file(&path).expect("remove");
    let removed = compose(&engine, &history, &w.project(), &["x.rs"]).expect("removed");
    assert!(
        removed.text.contains("Instructions removed: AGENTS.md")
            && removed.text.contains("no longer apply")
    );
    history.push(removed);
    assert!(
        compose(&engine, &history, &w.project(), &["x.rs"]).is_none(),
        "a removal is said once"
    );
    let state = visible_changes(&history);
    assert_eq!(
        state.values().next().map(|c| c.action),
        Some(ChangeAction::Remove)
    );
}

#[test]
fn a_baseline_made_under_other_rules_is_replaced_not_trusted() {
    let w = World::new("identity");
    w.scratch.write("proj/AGENTS.md", "RULES");
    let first = Engine::new(w.resolved(Config::default()));
    let mut history = vec![compose(&first, &[], &w.project(), &[]).expect("baseline")];
    let other = Engine::new(w.resolved(Config {
        max_bytes: 9000,
        ..Config::default()
    }));
    let replacement = compose(&other, &history, &w.project(), &[]).expect("replacement");
    assert!(
        replacement
            .text
            .contains("replaces all earlier workspace instruction baselines")
    );
    let source = replacement
        .context
        .as_ref()
        .and_then(ContextSource::instructions)
        .expect("source");
    assert!(source.baseline);
    history.push(replacement);
    assert!(visible_baseline(&history).is_some_and(|b| b.baseline_identity.is_some()));
    // The same rules again: the visible baseline is kept.
    assert!(compose(&other, &history, &w.project(), &[]).is_none());
}

#[test]
fn a_resumed_conversation_with_a_matching_baseline_says_nothing_until_something_changes() {
    let w = World::new("resume");
    w.scratch.write("proj/AGENTS.md", "RULES");
    let history = vec![
        compose(
            &Engine::new(w.resolved(Config::default())),
            &[],
            &w.project(),
            &[],
        )
        .expect("baseline"),
    ];
    // A new process: no cache, same rules.
    let fresh = Engine::new(w.resolved(Config::default()));
    assert!(compose(&fresh, &history, &w.project(), &[]).is_none());
}

#[test]
fn the_project_root_is_the_first_folder_with_a_marker_and_a_failed_probe_stops_the_walk() {
    let w = World::new("root");
    let deep = w.scratch.mkdir("proj/a/b");
    let markers = [".git".to_owned()];
    assert_eq!(
        find_project_root(&deep, &markers, &LocalFs).expect("found"),
        w.project()
    );
    let none = w.scratch.mkdir("loose/x");
    let found =
        find_project_root(&none, &[".definitely-not-here".to_owned()], &LocalFs).expect("walks");
    assert_eq!(
        found, none,
        "no marker anywhere: the working directory itself"
    );

    let fs = FakeFs::default();
    fs.markers.lock().unwrap().insert(
        PathBuf::from("/p/a/.git"),
        Err(std::io::ErrorKind::PermissionDenied),
    );
    let outcome = find_project_root(std::path::Path::new("/p/a"), &markers, &fs);
    assert_eq!(
        outcome.map_err(|e| e.kind()),
        Err(std::io::ErrorKind::PermissionDenied)
    );
}

#[test]
fn the_chain_and_the_folders_a_path_crosses_are_ordered_broad_to_specific() {
    let chain = ancestor_chain(std::path::Path::new("/r"), std::path::Path::new("/r/a/b"));
    assert_eq!(
        chain,
        [
            PathBuf::from("/r"),
            PathBuf::from("/r/a"),
            PathBuf::from("/r/a/b")
        ]
    );
    assert_eq!(
        descendant_dirs_between(std::path::Path::new("/r"), "a/b/c.rs"),
        [PathBuf::from("/r/a"), PathBuf::from("/r/a/b")]
    );
    assert!(descendant_dirs_between(std::path::Path::new("/r"), "c.rs").is_empty());
    assert!(descendant_dirs_between(std::path::Path::new("/r"), "/elsewhere/x.rs").is_empty());
    assert!(descendant_dirs_between(std::path::Path::new("/r/a"), "../b/c.rs").is_empty());
}

#[test]
fn a_group_that_cannot_be_checked_publishes_nothing_and_leaves_the_cache_alone() {
    let w = World::new("group-failure");
    let fs = FakeFs::default();
    let project = w.project();
    fs.markers
        .lock()
        .unwrap()
        .insert(project.join(".git"), Ok(true));
    let agents = project.join("AGENTS.md");
    let claude = project.join("CLAUDE.md");
    fs.file(&agents, "1", "ONE");
    fs.file(&claude, "1", "TWO");
    let engine = Engine::new(w.resolved(Config::default()));
    let baseline = engine
        .compose(&[], &project, &[], &fs)
        .expect("ok")
        .expect("baseline");
    assert!(baseline.text.contains("ONE") && baseline.text.contains("TWO"));
    let history = vec![baseline];

    // CLAUDE.md changes, but the probe of AGENTS.md (same group) fails: the group says nothing.
    fs.file(&claude, "2", "TWO CHANGED");
    fs.unavailable(&agents);
    assert!(
        engine
            .compose(&history, &project, &["x".to_string()], &fs)
            .expect("ok")
            .is_none()
    );
    // Once it can be checked, the change is reported, so nothing was lost.
    fs.file(&agents, "1", "ONE");
    let said = engine
        .compose(&history, &project, &["x".to_string()], &fs)
        .expect("ok")
        .expect("reported");
    assert!(said.text.contains("TWO CHANGED"));
    assert!(matches!(
        fs.probes.lock().unwrap().get(&agents),
        Some(Probe::Present { .. })
    ));
}

#[test]
fn a_change_batch_keeps_only_the_transitions_that_fit() {
    let item = |name: &str, action: ChangeAction, content: &str| ChangeItem {
        change: InstructionChange {
            action,
            scope: name.into(),
            path: name.into(),
            digest: Some("d".into()),
        },
        file: file(name, content),
    };
    let items = [
        item("a/AGENTS.md", ChangeAction::Set, &"a".repeat(80)),
        item("b/AGENTS.md", ChangeAction::Replace, &"b".repeat(80)),
    ];
    let (all, changes) = render_changes(&items, 100_000);
    assert_eq!(changes.len(), 2);
    assert!(
        all.contains("Additional instructions from: a/AGENTS.md")
            && all.contains("Updated instructions from: b/AGENTS.md")
    );
    let (small, kept) = render_changes(&items, 330);
    assert!(small.len() <= 330 && kept.len() < 2);
}
