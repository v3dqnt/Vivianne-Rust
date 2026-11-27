//! The reads, filters and traces over a folder of saved threads.

use vivianne_core::{Message, Mode, Role, ToolCall};

use super::filters::{EventFilter, Kind, Range, Surface, ThreadFilter};
use super::*;
use crate::store::{fork_to, save_to};

pub(crate) fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "viv-query-{name}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

pub(crate) fn thread(id: &str, workspace: &str, at: u64, texts: &[&str]) -> SavedThread {
    SavedThread {
        id: id.into(),
        workspace: workspace.into(),
        title: format!("title {id}"),
        title_source: "auto".into(),
        provider: "p".into(),
        model: "m".into(),
        mode: Mode::Ask,
        updated_at: at,
        history: texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                Message::new(role, *t).at(1000 + i as u64)
            })
            .collect(),
        parent: None,
    }
}

#[test]
fn threads_list_newest_first_and_filter_by_every_clause_together() {
    let dir = dir("list");
    save_to(&dir, &thread("a", "/p1", 10, &["one"])).expect("saved");
    save_to(&dir, &thread("b", "/p1", 30, &["two"])).expect("saved");
    save_to(&dir, &thread("c", "/p2", 20, &["three"])).expect("saved");
    std::fs::write(dir.join("broken.json"), "{ not json").expect("junk");
    let q = Query::new(&dir);
    let ids = |found: Vec<ThreadRecord>| found.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(
        ids(q.list_threads()),
        ["b", "c", "a"],
        "an unreadable file is left out"
    );
    let both = [
        ThreadFilter::Workspace(vec!["/p1".into()]),
        ThreadFilter::UpdatedAt(Range {
            from: Some(15),
            to: None,
        }),
    ];
    assert_eq!(ids(q.filter_threads(&both).expect("filtered")), ["b"]);
    let none = q
        .filter_threads(&[ThreadFilter::Id(vec![])])
        .expect("filtered");
    assert!(none.is_empty());
    let backwards = [ThreadFilter::UpdatedAt(Range {
        from: Some(9),
        to: Some(1),
    })];
    assert!(matches!(
        q.filter_threads(&backwards),
        Err(QueryError::InvalidFilter(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn events_filter_by_kind_text_position_and_whether_they_are_still_current() {
    let dir = dir("events");
    let long = thread(
        "t",
        "/w",
        5,
        &["fix the parser", "looking", "now the lexer", "done"],
    );
    save_to(&dir, &long).expect("saved");
    // Condensed: a shorter history makes the old one an earlier generation.
    let condensed = thread(
        "t",
        "/w",
        6,
        &["<compacted_history>old</compacted_history>"],
    );
    save_to(&dir, &condensed).expect("condensed");
    let q = Query::new(&dir);
    let all = q.list_events("t").expect("events");
    assert_eq!(all.len(), 5);
    assert_eq!(
        all.iter()
            .filter(|e| e.surface == Surface::Shadowed)
            .count(),
        4
    );
    let current = q
        .filter_events("t", &[EventFilter::Surface(vec![Surface::Current])])
        .expect("current");
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].record.kind, Kind::Checkpoint);
    let found = q
        .filter_events(
            "t",
            &[
                EventFilter::Text("THE   lexer".into()),
                EventFilter::Kind(vec![Kind::User]),
            ],
        )
        .expect("found");
    assert_eq!(
        (found.len(), found[0].record.seq, found[0].record.generation),
        (1, 2, 1)
    );
    let early = q
        .filter_events(
            "t",
            &[EventFilter::Seq(Range {
                from: None,
                to: Some(0),
            })],
        )
        .expect("early");
    assert_eq!(early.len(), 2, "seq 0 in both states");
    let window = Range {
        from: Some(1002),
        to: Some(1003),
    };
    let timed = q
        .filter_events("t", &[EventFilter::Time(window)])
        .expect("timed");
    assert_eq!(timed.len(), 2);
    let blank = q.filter_events("t", &[EventFilter::Text(" ".into())]);
    assert!(matches!(blank, Err(QueryError::InvalidFilter(_))));
    assert!(matches!(
        q.list_events("nope"),
        Err(QueryError::NotFound(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_event_comes_with_its_neighbours_within_a_bounded_window() {
    let dir = dir("window");
    save_to(&dir, &thread("t", "/w", 1, &["a", "b", "c", "d", "e"])).expect("saved");
    let q = Query::new(&dir);
    let w = q.read_event("t", 2, 1, 5).expect("window");
    assert_eq!(
        (
            w.start_seq,
            w.end_seq,
            w.events.len(),
            w.target.text.as_str()
        ),
        (1, 4, 4, "c")
    );
    let edge = q.read_event("t", 0, 3, 0).expect("window");
    assert_eq!((edge.start_seq, edge.end_seq), (0, 0));
    assert!(matches!(
        q.read_event("t", 9, 0, 0),
        Err(QueryError::NoSuchEvent { events: 5, .. })
    ));
    let wide = q.read_event("t", 0, READ_WINDOW_MAX + 1, 0);
    assert!(matches!(wide, Err(QueryError::InvalidFilter(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_branch_traces_to_its_parent_and_the_parent_to_its_branches() {
    let dir = dir("trace");
    save_to(&dir, &thread("root", "/w", 1, &["one", "a", "two", "b"])).expect("saved");
    let branch = fork_to(&dir, "root", 2).expect("forked");
    let q = Query::new(&dir);
    let up = q.trace_thread(&branch.id).expect("traced");
    let ancestors: Vec<&str> = up.ancestors.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ancestors, ["root"]);
    assert!(up.complete);
    let down = q.trace_thread("root").expect("traced");
    assert_eq!(down.descendants.len(), 1);
    assert_eq!(down.descendants[0].thread.id, branch.id);
    assert_eq!(
        down.descendants[0].thread.parent.as_ref().map(|p| p.turn),
        Some(2)
    );
    assert!(matches!(
        q.trace_thread("nope"),
        Err(QueryError::NotFound(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_events_links_and_a_checkpoints_earlier_state_are_traced() {
    let dir = dir("links");
    let mut t = thread("t", "/w", 1, &[]);
    let call = ToolCall {
        id: "c".into(),
        name: "read".into(),
        args: serde_json::json!({}),
    };
    t.history = vec![
        Message::new(Role::User, "go"),
        Message::assistant(String::new(), vec![call]),
        Message::tool_result("c", "text".to_owned()),
    ];
    save_to(&dir, &t).expect("saved");
    let q = Query::new(&dir);
    assert_eq!(
        q.trace_event("t", 2).expect("traced").trace.source_seqs,
        [1]
    );
    assert_eq!(
        q.trace_event("t", 1).expect("traced").trace.derived_seqs,
        [2]
    );
    t.history = vec![Message::new(
        Role::User,
        "<compacted_history>x</compacted_history>",
    )];
    save_to(&dir, &t).expect("condensed");
    let checkpoint = q.trace_event("t", 0).expect("traced");
    assert_eq!(
        (checkpoint.target.kind, checkpoint.replaces_generation),
        (Kind::Checkpoint, Some(1))
    );
    assert!(matches!(
        q.trace_event("t", 5),
        Err(QueryError::NoSuchEvent { .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
